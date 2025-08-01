use std::collections::HashMap;

use binance_sdk::spot::websocket_api::{
    ExchangeFiltersInner, ExchangeInfoResponseResult, ExchangeInfoResponseResultSymbolsInner,
};
use eyre::{eyre, OptionExt, Report, Result};
use itertools::Itertools;
use safe_math::safe;
use symm_core::core::{
    bits::{Amount, Symbol},
    decimal_ext::DecimalExt,
};
use tracing::warn;

pub enum Filter {
    PriceFilter {
        min_price: Amount,
        max_price: Amount,
        tick_size: Amount,
    },
    LotSize {
        min_quantity: Amount,
        max_quantity: Amount,
        step_size: Amount,
    },
    MinNotional {
        min_notional: Amount,
    },
    Notional {
        min_notional: Amount,
        max_notional: Amount,
    },
}

impl Filter {
    fn apply_filter(
        &self,
        price: &mut Amount,
        quantity: &mut Amount,
        allow_pad: bool,
    ) -> Result<()> {
        match self {
            Filter::PriceFilter {
                min_price,
                max_price,
                tick_size,
            } => {
                tracing::debug!(
                    "Applying PriceFilter: min={}, max={}, tick={}",
                    min_price,
                    max_price,
                    tick_size
                );
                let x = safe!(*price / *tick_size).ok_or_eyre("Math error")?;
                *price = if allow_pad {
                    safe!(x.ceil() * *tick_size).ok_or_eyre("Math error")?
                } else {
                    safe!(x.floor() * *tick_size).ok_or_eyre("Math error")?
                };
                if *price < *min_price {
                    if allow_pad {
                        warn!("Padding price to minimum price: {} => {}", price, min_price);
                        *price = *min_price;
                    } else {
                        Err(eyre!(
                            "Too small price {} (min_price = {})",
                            price,
                            min_price
                        ))?
                    }
                }
                if *price > *max_price {
                    warn!("Trimming price {} (max_price = {})", price, max_price);
                    *price = *max_price;
                }
            }
            Filter::LotSize {
                min_quantity,
                max_quantity,
                step_size,
            } => {
                tracing::debug!(
                    "Applying LotSize: min={}, max={}, step={}",
                    min_quantity,
                    max_quantity,
                    step_size
                );
                let x = safe!(*quantity / *step_size).ok_or_eyre("Math error")?;
                *quantity = if allow_pad {
                    safe!(x.ceil() * *step_size).ok_or_eyre("Math error")?
                } else {
                    safe!(x.floor() * *step_size).ok_or_eyre("Math error")?
                };
                if *quantity < *min_quantity {
                    if allow_pad {
                        warn!(
                            "Padding quantity to minimum quantity: {} => {}",
                            quantity, min_quantity
                        );
                    } else {
                        *quantity = *min_quantity;
                        Err(eyre!(
                            "Too small quantity {} (min_quantity = {})",
                            quantity,
                            min_quantity
                        ))?
                    }
                }
                if *quantity > *max_quantity {
                    warn!(
                        "Trimming quantity {} (max_quantity = {})",
                        quantity, max_quantity
                    );
                    *quantity = *max_quantity;
                }
            }
            Filter::MinNotional { min_notional } => {
                tracing::debug!("Applying MinNotional: min={}", min_notional);
                let min_quantity = safe!(*min_notional / *price).ok_or_eyre("Math error")?;
                if allow_pad {
                    warn!(
                        "Padding quantity to match minimum notional: {} => {} (min_notional = {})",
                        quantity, min_quantity, min_notional
                    );
                    *quantity = min_quantity;
                } else {
                    if *quantity < min_quantity {
                        Err(eyre!(
                            "Too small quantity {} for price {} (min_notional = {})",
                            quantity,
                            price,
                            min_notional
                        ))?
                    }
                }
            }
            Filter::Notional {
                min_notional,
                max_notional,
            } => {
                tracing::debug!(
                    "Applying Notional: min={}, max={}",
                    min_notional,
                    max_notional
                );
                let min_quantity = safe!(*min_notional / *price).ok_or_eyre("Math error")?;
                let max_quantity = safe!(*max_notional / *price).ok_or_eyre("Math error")?;
                if *quantity < min_quantity {
                    if allow_pad {
                        warn!(
                        "Padding quantity to match minimum notional: {} => {} (min_notional = {})",
                        quantity, min_quantity, min_notional
                    );
                        *quantity = min_quantity;
                    } else {
                        Err(eyre!(
                            "Too small quantity {} for price {} (min_notional = {})",
                            quantity,
                            price,
                            min_notional
                        ))?
                    }
                }
                if *quantity > max_quantity {
                    warn!(
                        "Trimming quantity {} for price {} (max_notional = {})",
                        quantity, price, max_notional
                    );
                    *quantity = max_quantity;
                }
            }
        }
        Ok(())
    }

    fn make_from(value: ExchangeFiltersInner) -> Result<Option<Self>> {
        let filter_type = value.filter_type.ok_or_eyre("Missing filter_type")?;

        let filter = match filter_type.as_str() {
            "PRICE_FILTER" => Some(Filter::PriceFilter {
                min_price: Amount::from_str_exact(
                    &value.min_price.ok_or_eyre("Missing min_price")?,
                )?,
                max_price: Amount::from_str_exact(
                    &value.max_price.ok_or_eyre("Missing max_price")?,
                )?,
                tick_size: Amount::from_str_exact(
                    &value.tick_size.ok_or_eyre("Missing tick_size")?,
                )?,
            }),
            "LOT_SIZE" => Some(Filter::LotSize {
                min_quantity: Amount::from_str_exact(
                    &value.min_qty.ok_or_eyre("Missing min_qty")?,
                )?,
                max_quantity: Amount::from_str_exact(
                    &value.max_qty.ok_or_eyre("Missing max_qty")?,
                )?,
                step_size: Amount::from_str_exact(
                    &value.step_size.ok_or_eyre("Missing step_size")?,
                )?,
            }),
            "MIN_NOTIONAL" => Some(Filter::MinNotional {
                min_notional: Amount::from_str_exact(
                    &value.min_notional.ok_or_eyre("Missing min_notional")?,
                )?,
            }),
            "NOTIONAL" => Some(Filter::Notional {
                min_notional: Amount::from_str_exact(
                    &value.min_notional.ok_or_eyre("Missing min_notional")?,
                )?,
                max_notional: Amount::from_str_exact(
                    &value.max_notional.ok_or_eyre("Missing max_notional")?,
                )?,
            }),
            _ => None,
        };

        Ok(filter)
    }
}

pub struct AssetInfo {
    symbol: Symbol,
    precision: u32,
    commission_presision: u32,
}

impl AssetInfo {
    pub fn get_symbol(&self) -> &Symbol {
        &self.symbol
    }

    pub fn get_commission_precision(&self) -> u32 {
        self.commission_presision
    }
}

pub struct MarketInfo {
    symbol: Symbol,
    base: AssetInfo,
    quote: AssetInfo,
    filters: Vec<Filter>,
}

impl MarketInfo {
    pub fn get_symbol(&self) -> &Symbol {
        &self.symbol
    }

    pub fn treat_price_quantity(
        &self,
        price: &mut Amount,
        quantity: &mut Amount,
        allow_pad: bool,
    ) -> Result<()> {
        price.rescale(self.quote.precision);
        quantity.rescale(self.base.precision);

        tracing::debug!(
            "Applying filters to: sybol={}, price={}, quantity={}",
            self.symbol,
            price,
            quantity
        );

        let (_, bad): ((), Vec<_>) = self
            .filters
            .iter()
            .map(|f| f.apply_filter(price, quantity, allow_pad))
            .partition_result();

        if !bad.is_empty() {
            Err(eyre!(
                "Error while applying filters: {}",
                bad.into_iter().map(|e| format!("{:?}", e)).join(",")
            ))?;
        }

        Ok(())
    }
}

impl TryFrom<ExchangeInfoResponseResultSymbolsInner> for MarketInfo {
    type Error = Report;

    fn try_from(symbol: ExchangeInfoResponseResultSymbolsInner) -> Result<Self> {
        let market_symbol =
            Symbol::from(symbol.symbol.ok_or_eyre("Missing symbol")?.to_uppercase());

        let base_symbol = Symbol::from(
            symbol
                .base_asset
                .ok_or_else(|| eyre!("Missing base_asset for {}", market_symbol))?
                .to_uppercase(),
        );

        let quote_symbol = Symbol::from(
            symbol
                .quote_asset
                .ok_or_else(|| eyre!("Missing quote_asset for {}", market_symbol))?
                .to_uppercase(),
        );

        let filters = symbol.filters.map_or(vec![], |filters| {
            let (good, bad): (Vec<_>, Vec<_>) = filters
                .into_iter()
                .map(|f| Filter::make_from(f))
                .filter_map_ok(|f| f)
                .partition_result();

            if !bad.is_empty() {
                tracing::warn!(
                    "Failed to setup some filters: {}",
                    bad.into_iter().map(|e| format!("{:?}", e)).join(",")
                );
            }
            good
        });

        Ok(MarketInfo {
            symbol: market_symbol.clone(),
            base: AssetInfo {
                symbol: base_symbol.clone(),
                precision: symbol.base_asset_precision.ok_or_else(|| {
                    eyre!(
                        "Missing base_asset_precision for {} in {}",
                        base_symbol,
                        market_symbol
                    )
                })? as u32,
                commission_presision: symbol.base_commission_precision.ok_or_else(|| {
                    eyre!(
                        "Missing base_commission_precision for {} in {}",
                        base_symbol,
                        market_symbol
                    )
                })? as u32,
            },
            quote: AssetInfo {
                symbol: quote_symbol.clone(),
                precision: symbol.quote_asset_precision.ok_or_else(|| {
                    eyre!(
                        "Missing quote_asset_precision for {} in {}",
                        quote_symbol,
                        market_symbol
                    )
                })? as u32,
                commission_presision: symbol.quote_commission_precision.ok_or_else(|| {
                    eyre!(
                        "Missing quote_commission_precision for {} in {}",
                        quote_symbol,
                        market_symbol
                    )
                })? as u32,
            },
            filters,
        })
    }
}

pub struct TradingMarkets {
    markets: HashMap<Symbol, MarketInfo>,
}

impl TradingMarkets {
    pub fn new() -> Self {
        Self {
            markets: HashMap::new(),
        }
    }

    pub fn get_markets(&self) -> &HashMap<Symbol, MarketInfo> {
        &self.markets
    }

    pub fn treat_price_quantity(
        &self,
        symbol: &Symbol,
        price: &mut Amount,
        quantity: &mut Amount,
        allow_pad: bool,
    ) -> Result<()> {
        let market_info = self
            .markets
            .get(symbol)
            .ok_or_else(|| eyre!("Cannot find market info for {}", symbol))?;

        market_info.treat_price_quantity(price, quantity, allow_pad)?;

        // Do second round of treating to ensure all filters are happy
        market_info.treat_price_quantity(price, quantity, allow_pad)
    }

    pub fn ingest_exchange_info(
        &mut self,
        exchange_info: Box<ExchangeInfoResponseResult>,
    ) -> Result<()> {
        //todo!("Use exchange info: symbols.filters where filter_type is PRICE_FILTER | LOT_SIZE");
        if let Some(exchange_info_symbols) = exchange_info.symbols {
            for exchange_info_symbol in exchange_info_symbols {
                match MarketInfo::try_from(exchange_info_symbol) {
                    Ok(market_info) => {
                        self.markets.insert(market_info.symbol.clone(), market_info);
                    }
                    Err(err) => {
                        tracing::debug!("Failed to ingest exchange info: {:?}", err);
                    }
                }
            }
        }

        Ok(())
    }
}
