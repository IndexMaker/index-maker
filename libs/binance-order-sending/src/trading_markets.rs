use std::collections::HashMap;

use binance_sdk::spot::websocket_api::{
    ExchangeInfoResponseResult, ExchangeInfoResponseResultSymbolsInner,
};
use eyre::{eyre, OptionExt, Report, Result};
use symm_core::core::bits::{Amount, Symbol};

struct AssetInfo {
    symbol: Symbol,
    precision: u32,
    commission_presision: u32,
}

struct MarketInfo {
    symbol: Symbol,
    base: AssetInfo,
    quote: AssetInfo,
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

    pub fn treat_price_quantity(
        &self,
        symbol: &Symbol,
        price: &mut Amount,
        quantity: &mut Amount,
    ) -> Result<()> {
        let market_info = self.markets
            .get(symbol)
            .ok_or_else(|| eyre!("Cannot find market info for {}", symbol))?;

        price.rescale(market_info.quote.precision);
        quantity.rescale(market_info.base.precision);

        Ok(())
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
