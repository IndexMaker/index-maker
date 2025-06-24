use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    ops::Deref,
    sync::Arc,
};

use eyre::{eyre, OptionExt, Result};
use itertools::{Either, Itertools};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use safe_math::safe;

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, PriceType, Side, Symbol},
    decimal_ext::DecimalExt,
};

use crate::{
    index::basket::Basket,
    solver::{
        solver::{
            CollateralManagement, EngagedSolverOrders, SolveEngagementsResult, SolveQuotesResult,
            SolverOrderEngagement, SolverStrategy, SolverStrategyHost,
        },
        solver_order::{SolverOrder, SolverOrderStatus},
        solver_quote::{SolverQuote, SolverQuoteStatus},
    },
};

struct SimpleSolverEngagements {
    baskets: HashMap<Symbol, Arc<Basket>>,
    asset_price_limits: HashMap<Symbol, Amount>,
    index_price_limits: HashMap<Symbol, Amount>,
    quantity_contributions: HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
    contribution_fractions: HashMap<(Address, ClientOrderId), HashMap<Symbol, Amount>>,
}

pub struct SimpleSolver {
    /// When testing for liquidity, this sets the limit as to how far from top
    /// of the book we want to go, e.g. 0.01 (max price move = 1%)
    price_threshold: Amount,
    /// When calculating quantity that collateral can buy, we need to factor in
    /// fees, e.g. 1.001 (max fee = 0.1%)
    fee_factor: Amount,
    /// Cap the amount of collateral an order can potentially consume in one batch
    max_order_volley_size: Amount,
    /// Cap the total amount of collateral all orders in the batch can potentially consume
    max_volley_size: Amount,
}

impl SimpleSolver {
    pub fn new(
        price_threshold: Amount,
        fee_factor: Amount,
        max_order_volley_size: Amount,
        max_volley_size: Amount,
    ) -> Self {
        Self {
            price_threshold,
            fee_factor,
            max_order_volley_size,
            max_volley_size,
        }
    }

    /// Apply a function to each Index order in the batch, and collect results.
    ///
    fn scan_order_batch<OrderPtr, UpRead, SetOrderStatusFn, ScanFn, ScanRet>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        error_action: &str,
        error_status: SolverOrderStatus,
        scan_fn: ScanFn,
    ) -> (HashMap<(Address, ClientOrderId), ScanRet>, Vec<OrderPtr>)
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
        ScanFn: Fn(&mut UpRead) -> Result<ScanRet>,
    {
        let mut good = HashMap::new();
        let mut bad = Vec::new();

        locked_order_batch.retain_mut(|(order_ptr, order_upread)| match scan_fn(order_upread) {
            Ok(scan_ret) => {
                good.insert(
                    (order_upread.address, order_upread.client_order_id.clone()),
                    scan_ret,
                );
                true
            }
            Err(err) => {
                eprintln!(
                    "(simple-solver) Error while {} for IndexOrder {}: {:?}",
                    error_action, order_upread.client_order_id, err
                );
                set_order_status(order_upread, error_status);
                bad.push(order_ptr.clone());
                false
            }
        });

        (good, bad)
    }

    /// Scan order batch for baskets
    ///
    /// Finds a basket for each Index Order, and
    /// creates a mapping Index Symbol => Basket.
    fn get_baskets<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
    ) -> Result<(HashMap<Symbol, Arc<Basket>>, Vec<OrderPtr>)>
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        let mut baskets = HashMap::new();
        let mut bad = Vec::new();

        locked_order_batch.retain_mut(|(order_ptr, order_upread)| {
            match (|| -> Result<()> {
                let symbol = order_upread.symbol.clone();
                match strategy_host.get_basket(&symbol) {
                    Some(basket) => {
                        baskets.entry(symbol.clone()).or_insert(basket);
                    }
                    None => {
                        set_order_status(order_upread, SolverOrderStatus::InvalidSymbol);
                        Err(eyre!(
                            "Index Order {} has invalid symbol {}",
                            order_upread.client_order_id,
                            symbol
                        ))?;
                    }
                }

                Ok(())
            })() {
                Ok(()) => true,
                Err(_) => {
                    bad.push(order_ptr.clone());
                    false
                }
            }
        });

        Ok((baskets, bad))
    }

    /// Scan quotes for baskets
    ///
    /// Finds a basket for each Index Quote, and
    /// creates a mapping Index Symbol => Basket.
    fn get_baskets_from_quotes<QuotePtr, UpRead, SetQuoteStatusFn>(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        locked_quote_batch: &mut Vec<(QuotePtr, UpRead)>,
        set_quote_status: SetQuoteStatusFn,
    ) -> Result<(HashMap<Symbol, Arc<Basket>>, Vec<QuotePtr>)>
    where
        QuotePtr: Clone,
        UpRead: Deref<Target = SolverQuote>,
        SetQuoteStatusFn: Fn(&mut UpRead, SolverQuoteStatus),
    {
        let mut baskets = HashMap::new();
        let mut bad = Vec::new();

        locked_quote_batch.retain_mut(|(quote_ptr, quote_upread)| {
            match (|| -> Result<()> {
                let symbol = quote_upread.symbol.clone();
                match strategy_host.get_basket(&symbol) {
                    Some(basket) => {
                        baskets.entry(symbol.clone()).or_insert(basket);
                    }
                    None => {
                        set_quote_status(quote_upread, SolverQuoteStatus::InvalidSymbol);
                        Err(eyre!(
                            "Index Quote {} has invalid symbol {}",
                            quote_upread.client_quote_id,
                            symbol
                        ))?;
                    }
                }

                Ok(())
            })() {
                Ok(()) => true,
                Err(_) => {
                    bad.push(quote_ptr.clone());
                    false
                }
            }
        });

        Ok((baskets, bad))
    }

    /// Obtain price limits for a set of assets
    ///
    /// Find prices of assets given, and offset them into the depth of the Side
    /// by the Price Threshold, and create a mapping Asset Symbol => Price
    /// Limit.
    ///
    /// We will use these offset prices (higher if we're buying / lower if we're
    /// sellling) to calculate the maximum amount of Collateral that orders in
    /// the batch will consume when sent to exchange. This means that potentially
    /// those orders will consume less Collateral, as transactions will be made
    /// at slightly better prices. Price Threshold controlls how much the amount
    /// can deviate.
    ///
    /// Note that we cannot send an order with higher Volley Size (Limit Price *
    /// Order Quantity) than the amount of Collateral + Fees. Also note that
    /// Volley Size, which depends on parameters of an order tha we send to
    /// exchange is always higher than amount of Collateral that we will
    /// consume, which depends on execution price. Exchange will never execute
    /// more quantity than ordered, but it will execute at different prices up
    /// to the limit.
    ///
    fn get_asset_price_limits(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        side: Side,
        symbols: &Vec<Symbol>,
    ) -> Result<(HashMap<Symbol, Amount>, Vec<Symbol>)> {
        // Depending on order side, we want to cross the book on opposite side:w
        let (price_type, price_factor) = match side {
            Side::Buy => (
                PriceType::BestAsk,
                safe!(Amount::ONE + self.price_threshold)
                    .ok_or_eyre("Math Problem: Failed to compute price factor for Ask")?,
            ),
            Side::Sell => (
                PriceType::BestBid,
                safe!(Amount::ONE - self.price_threshold)
                    .ok_or_eyre("Math Problem: Failed to compute price factor for Bid")?,
            ),
        };

        // Get top of the book prices
        let get_prices = strategy_host.get_prices(price_type, &symbols);

        let prices_len = get_prices.prices.len();

        for (k, v) in &get_prices.prices {
            println!("(simple-solver) Price: {:?} {} {}", side, k, v);
        }

        let price_limits: HashMap<_, _> = get_prices
            .prices
            .into_iter()
            .map_while(|(symbol, price)| Some((symbol.clone(), safe!(price * price_factor)?)))
            .collect();

        if price_limits.len() != prices_len {
            Err(eyre!("Math Problem: Failed to compute price limits"))?;
        }

        for (k, v) in &price_limits {
            println!("(simple-solver) Price Limit: {} {}", k, v);
        }

        Ok((price_limits, get_prices.missing_symbols))
    }

    /// Calculate prices of the Indexes using given prices of the Assets.
    ///
    /// Based on the Asset prices we calculate price of each Index, by summing
    /// up Price * Quantity of each Asset in a Basket, for each Index.
    ///
    fn get_index_price(
        &self,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        asset_prices: &HashMap<Symbol, Amount>,
    ) -> (HashMap<Symbol, Amount>, Vec<Symbol>) {
        let (index_prices, bad): (Vec<_>, Vec<_>) = baskets
            .iter()
            .map(|(symbol, basket)| (symbol.clone(), basket.get_current_price(asset_prices)))
            .partition_map(|(symbol, index_price_result)| match index_price_result {
                Ok(price) => {
                    println!("(simple-solver) Index Price: {} {}", symbol, price);
                    Either::Left((symbol, price))
                }
                Err(err) => {
                    eprintln!(
                        "(simple-solver) Failed to compute index price for {}: {:?}",
                        symbol, err
                    );
                    Either::Right(symbol)
                }
            });

        (HashMap::from_iter(index_prices.into_iter()), bad)
    }

    /// Calculate quantity of An Index that fits within Collateral given, and
    /// then calculate quantity of each Asset in the Basket of an Index.
    ///
    /// We factor in the potential fees using Fee Factor, which is equal to
    /// (1.0 + Fee Rate), and after this adjustment we fit Index quantity into
    /// the remainng portion of Collateral. Once we know how much Index will fit
    /// into Collateral (including potential fees), we calculate amounts of
    /// Assets that will make up this Index in that quantity.
    fn compute_quantities_for_order<UpRead>(
        &self,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        index_price_limits: &HashMap<Symbol, Amount>,
        order_upread: &UpRead,
    ) -> Result<(Amount, HashMap<Symbol, Amount>)>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let client_order_id = &order_upread.client_order_id;
        let index_symbol = &order_upread.symbol;
        let collateral_amount = order_upread.remaining_collateral;

        let basket = baskets
            .get(index_symbol)
            .ok_or_else(|| eyre!("Basket not found for {} {}", client_order_id, index_symbol))?;

        let index_price = *index_price_limits.get(index_symbol).ok_or_else(|| {
            eyre!(
                "Can't find index price for {} {}",
                client_order_id,
                index_symbol
            )
        })?;

        let collateral_available =
            safe!(safe!(collateral_amount + order_upread.collateral_carried) / self.fee_factor)
                .ok_or_eyre("Fee factor multiplication error")?;

        // Cap order volley size
        let collateral_usable = collateral_available.min(self.max_order_volley_size);

        let index_order_quantity = safe!(collateral_usable / index_price)
            .ok_or_eyre("Index order quantity computation error")?;

        println!(
            "(simple-solver) Collateral to Quantity for Index Order: {} c={:0.5} cc={:0.5} ca={:0.5} cu={:0.5} p={:0.5} q={:0.5} ff={:0.5}",
            client_order_id,
            collateral_amount,
            order_upread.collateral_carried,
            collateral_available,
            collateral_usable,
            index_price,
            index_order_quantity,
            self.fee_factor
        );

        let asset_quantities: HashMap<_, _> = basket
            .basket_assets
            .iter()
            .map_while(|basket_asset| {
                let asset_symbol = &basket_asset.weight.asset.name;
                let asset_quantity = safe!(basket_asset.quantity * index_order_quantity)?;
                println!(
                    "(simple-solver) Asset Quantity for Index Order: {} {} q={:0.5} baq={:0.5} oq={:0.5}",
                    client_order_id,
                    asset_symbol,
                    asset_quantity,
                    basket_asset.quantity,
                    index_order_quantity
                );
                Some((asset_symbol.clone(), asset_quantity))
            })
            .collect();

        if asset_quantities.len() != basket.basket_assets.len() {
            Err(eyre!(
                "Failed to compute asset quantities for {}",
                client_order_id
            ))?;
        }

        Ok((index_order_quantity, asset_quantities))
    }

    fn cap_volley_size_for_order<UpRead>(
        &self,
        volley_fraction: Amount,
        order_quantities: &HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
        order_upread: &UpRead,
    ) -> Result<(Amount, HashMap<Symbol, Amount>)>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let address = &order_upread.address;
        let client_order_id = order_upread.client_order_id.clone();

        let (order_quantity, asset_quantities) = order_quantities
            .get(&(*address, client_order_id.clone()))
            .ok_or_eyre("Cannot find index order quantity")?;

        let order_quantity = *order_quantity;
        let capped_order_quantity = safe!(order_quantity * volley_fraction)
            .ok_or_eyre("Cannot calculate capped order quantity")?;

        println!(
            "(simple-solver) Capping Volley Size for Index Order: {} oq={:0.5} coq={:0.5}",
            client_order_id, order_quantity, capped_order_quantity
        );

        let mut capped_asset_quantities = HashMap::new();

        for (asset_symbol, asset_quantity) in asset_quantities {
            let asset_quantity = *asset_quantity;
            let capped_asset_quantity = safe!(asset_quantity * volley_fraction)
                .ok_or_eyre("Failed to compute caped asset quantity")?;

            capped_asset_quantities.insert(asset_symbol.clone(), capped_asset_quantity);

            println!(
                "(simple-solver) Capping Volley Size for Asset: {} aq={:0.5} caq={:0.5}",
                asset_symbol, asset_quantity, capped_asset_quantity
            );
        }

        Ok((capped_order_quantity, capped_asset_quantities))
    }

    /// Calculate total sum of quantities per each Asset across all Index orders
    /// in the batch.
    ///
    /// We sum up values per each Asset, so that we wil be then able to compute
    /// contribution fraction for each Index order, i.e. to tell how much of
    /// ordered Asset quantity will be distributed to that Index Order once we
    /// receive fills.
    ///
    /// Note that this is important, as we don't send to exchange individual
    /// orders for each Asset of each Index order. Instead we are groupping
    /// Assets from all Index orders in the batch, so that a single order that
    /// we send for an Asset includes quantity for all Index orders in the batch
    /// that need that Asset. Note also that not all Index orders in the batch
    /// need same Assets, so we group by Asset, and then if two or more Index
    /// orders need that same Asset, we will split quantity of that Asset
    /// between those Index orders. On top of that we must distribute quantity
    /// of all Assets for Index order in exact proportion that is defined by
    /// th Basket of the Index. That is why we will be finding maximum possible
    /// quantity of each Index order that would fit in those criteria. To find
    /// maximum quanity we take minimum of possible maximums.
    ///
    fn summarise_asset_quanties<Key>(
        &self,
        order_quantities: &HashMap<Key, (Amount, HashMap<Symbol, Amount>)>,
    ) -> Result<HashMap<Symbol, Amount>> {
        let mut result = HashMap::new();

        for (_, (_, map)) in order_quantities.iter() {
            for (asset_symbol, &quantity) in map.iter() {
                match result.entry(asset_symbol.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(quantity);
                    }
                    Entry::Occupied(mut entry) => {
                        let current = *entry.get();
                        let value = safe!(current + quantity)
                            .ok_or_eyre("Math problem while summing up quantities")?;
                        entry.insert(value);
                    }
                }
            }
        }

        Ok(result)
    }

    fn compute_volley_size(
        &self,
        asset_quantites: &HashMap<Symbol, Amount>,
        asset_prices: &HashMap<Symbol, Amount>,
    ) -> Result<Amount> {
        let mut total_volley_size = Amount::ZERO;
        for (symbol, &quantity) in asset_quantites {
            let price = *asset_prices
                .get(&symbol)
                .ok_or_else(|| eyre!("Cannot find price for an asset {}", symbol))?;

            let volley_size = safe!(price * quantity)
                .ok_or_else(|| eyre!("Cannot calculate volley size for an asset {}", symbol))?;

            total_volley_size = safe!(total_volley_size + volley_size).ok_or_else(|| {
                eyre!("Cannot calculate total volley size for an asset {}", symbol)
            })?;
        }
        Ok(total_volley_size)
    }

    /// Calculate maximum contribution of each Index order that fits into
    /// Liquidity.
    ///
    /// We take total sum of quantites per each Asset, we take Index order
    /// quantity for each Asset, and we divide by total sum, so that we know
    /// contribution fraction. Then we multiply by available Liquidity for that
    /// Asset, we we have quantity that together with quantites from other Index
    /// orders in the batch will fit into Liquidity. Then we calculate this
    /// Index order quantity that would be needed to obtain that ammount of this
    /// Asset, and while we repeat this for each asset, we take the minimum
    /// value, so that that this Index order all its Assets will fit within
    /// allocated portion of their Liquidity. Should the value come up higher
    /// than initially calculated Index order quantity, then we cap it to that
    /// initial value.
    fn compute_contribution_for_order<UpRead>(
        &self,
        total_asset_liquidity: &HashMap<Symbol, Amount>,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
        order_upread: &UpRead,
    ) -> Result<Amount>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let address = &order_upread.address;
        let client_order_id = &order_upread.client_order_id;

        let (order_quantity, asset_quanties) = order_quantities
            .get(&(*address, client_order_id.clone()))
            .ok_or_else(|| eyre!("Missing asset quantities for {}", client_order_id))?;

        let order_quantity = *order_quantity;
        let mut fitting_order_quantity = order_quantity;

        // We want to check order quantity against available liquidity, or more
        // precisely against a fraction of available liquidity, so that other
        // orders in the batch can also have some of it. This is simple solver
        // and it only does allocation proportional to order quantity possible
        // within given collateral.
        for (asset_symbol, &asset_quantity) in asset_quanties.iter() {
            let total_asset_liquidity =
                *total_asset_liquidity.get(&asset_symbol).ok_or_else(|| {
                    eyre!(
                        "Failed to obtain total asset liquidity for {} {}",
                        client_order_id,
                        asset_symbol
                    )
                })?;

            let total_asset_quantity =
                *total_asset_quantities.get(&asset_symbol).ok_or_else(|| {
                    eyre!(
                        "Failed to obtain total asset quantity for {} {}",
                        client_order_id,
                        asset_symbol
                    )
                })?;

            let asset_contribution_fraction = safe!(asset_quantity / total_asset_quantity)
                .ok_or_else(|| {
                    eyre!(
                        "Failed to compute asset contribution fraction for {} {}",
                        client_order_id,
                        asset_symbol
                    )
                })?;

            let asset_liquidity_contribution =
                safe!(total_asset_liquidity * asset_contribution_fraction).ok_or_else(|| {
                    eyre!(
                        "Failed to compute asset liquidity contribution for {} {}",
                        client_order_id,
                        asset_symbol
                    )
                })?;

            let possible_order_quantity =
                safe!(safe!(asset_liquidity_contribution * order_quantity) / asset_quantity)
                    .ok_or_else(|| {
                        eyre!(
                            "Failed to compute possible order quantity for {} {}",
                            client_order_id,
                            asset_symbol
                        )
                    })?;

            // Choose smaller one, so that order will leave some liquidity for
            // other orders in the batch.  Note that liquidity is computed based
            // on price threshold, so that we don't wipe too many levels.
            fitting_order_quantity = fitting_order_quantity.min(possible_order_quantity);

            println!(
                "(simple-solver) Fitting Quantity for Index Order: {} {} {:0.5} tal={:0.5} taq={:0.5} acf={:0.5} alc={:0.5} poq={:0.5}",
                client_order_id,
                asset_symbol,
                fitting_order_quantity,
                total_asset_liquidity,
                total_asset_quantity,
                asset_contribution_fraction,
                asset_liquidity_contribution,
                possible_order_quantity
            );
        }

        Ok(fitting_order_quantity)
    }

    /// Calculate quantity of each Asset for given quantity of an Index order.
    ///
    /// Once we have found maximum quantity of the Index order fitting into
    /// assigned portion of liquidity of all Assets involved, we then need to
    /// recalculate quantity of each Asset, so that we know how much of each
    /// Asset we want to order on exchange.
    ///
    fn compute_asset_contributions_for_order<UpRead>(
        &self,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        order_contributions: &HashMap<(Address, ClientOrderId), Amount>,
        order_upread: &UpRead,
    ) -> Result<(Amount, HashMap<Symbol, Amount>)>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let address = &order_upread.address;
        let client_order_id = &order_upread.client_order_id;
        let index_symbol = &order_upread.symbol;

        let mut asset_contributions = HashMap::new();

        let order_quantity = *order_contributions
            .get(&(*address, client_order_id.clone()))
            .ok_or_else(|| eyre!("Missing order contribution for {}", client_order_id))?;

        let basket = baskets
            .get(index_symbol)
            .ok_or_else(|| eyre!("Missing basket for {} {}", client_order_id, index_symbol))?;

        for basket_asset in basket.basket_assets.iter() {
            let asset_symbol = &basket_asset.weight.asset.name;
            let asset_quantity =
                safe!(basket_asset.quantity * order_quantity).ok_or_else(|| {
                    eyre!(
                        "Failed to compute asset contribution quantity for {} {}",
                        client_order_id,
                        asset_symbol,
                    )
                })?;

            asset_contributions.insert(asset_symbol.clone(), asset_quantity);
        }

        Ok((order_quantity, asset_contributions))
    }

    /// Calculate contribution fractions for each Asset for each Index order.
    ///
    /// Once we know how much od each Asset we want to order for each Index
    /// order, we need to know how to distribute fills, and for that we need
    /// contribution fraction, so that when fill for an Asset order arrives,
    /// we split it between Index orders according to contribution fraction.
    ///
    fn compute_asset_contribution_fractions_for_order<UpRead>(
        &self,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
        order_upread: &UpRead,
    ) -> Result<HashMap<Symbol, Amount>>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let address = &order_upread.address;
        let client_order_id = &order_upread.client_order_id;

        let (_, asset_quanties) = order_quantities
            .get(&(*address, client_order_id.clone()))
            .ok_or_else(|| eyre!("Missing asset quantities for {}", client_order_id))?;

        let mut asset_contribution_fractions = HashMap::new();

        // We want to check order quantity against available liquidity, or more
        // precisely against a fraction of available liquidity, so that other
        // orders in the batch can also have some of it. This is simple solver
        // and it only does allocation proportional to order quantity possible
        // within given collateral.
        for (asset_symbol, &asset_quantity) in asset_quanties.iter() {
            let total_asset_quantity =
                *total_asset_quantities.get(&asset_symbol).ok_or_else(|| {
                    eyre!(
                        "Failed to obtain total asset quantity for {} {}",
                        client_order_id,
                        asset_symbol
                    )
                })?;

            let asset_contribution_fraction = safe!(asset_quantity / total_asset_quantity)
                .ok_or_else(|| {
                    eyre!(
                        "Failed to compute asset contribution fraction for {} {}",
                        client_order_id,
                        asset_symbol
                    )
                })?;

            asset_contribution_fractions.insert(asset_symbol.clone(), asset_contribution_fraction);

            println!(
                "(simple-solver) Asset Fractions for Index Order: {} {} taq={:0.5} acf={:0.5}",
                client_order_id, asset_symbol, total_asset_quantity, asset_contribution_fraction,
            );
        }

        Ok(asset_contribution_fractions)
    }

    fn compute_quantities_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        index_price_limits: &HashMap<Symbol, Amount>,
    ) -> (
        HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
        Vec<OrderPtr>,
    )
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        self.scan_order_batch(
            locked_order_batch,
            set_order_status,
            "computing quantities",
            SolverOrderStatus::MathOverflow,
            |order_upread| {
                self.compute_quantities_for_order(baskets, index_price_limits, order_upread)
            },
        )
    }

    fn cap_volley_size<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        total_volley_size: Amount,
        order_quantities: HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
    ) -> Result<(
        HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
        Vec<OrderPtr>,
    )>
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        if total_volley_size < self.max_volley_size {
            return Ok((order_quantities, Vec::new()));
        }

        let volley_fraction = safe!(self.max_volley_size / total_volley_size)
            .ok_or_eyre("Failed to compute volley fraction")?;

        Ok(self.scan_order_batch(
            locked_order_batch,
            set_order_status,
            "capping volley size",
            SolverOrderStatus::MathOverflow,
            |order_upread| {
                self.cap_volley_size_for_order(volley_fraction, &order_quantities, order_upread)
            },
        ))
    }

    fn compute_contributions_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        total_asset_liquidity: &HashMap<Symbol, Amount>,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
    ) -> (HashMap<(Address, ClientOrderId), Amount>, Vec<OrderPtr>)
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        self.scan_order_batch(
            locked_order_batch,
            set_order_status,
            "computing contributions",
            SolverOrderStatus::MathOverflow,
            |order_upread| {
                self.compute_contribution_for_order(
                    total_asset_liquidity,
                    total_asset_quantities,
                    order_quantities,
                    order_upread,
                )
            },
        )
    }

    fn compute_asset_contributions_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        order_contributions: &HashMap<(Address, ClientOrderId), Amount>,
    ) -> (
        HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
        Vec<OrderPtr>,
    )
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        self.scan_order_batch(
            locked_order_batch,
            set_order_status,
            "computing asset contributions",
            SolverOrderStatus::MathOverflow,
            |order_upread| {
                self.compute_asset_contributions_for_order(
                    baskets,
                    order_contributions,
                    order_upread,
                )
            },
        )
    }

    fn compute_asset_contribution_fractions_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<(Address, ClientOrderId), (Amount, HashMap<Symbol, Amount>)>,
    ) -> (
        HashMap<(Address, ClientOrderId), HashMap<Symbol, Amount>>,
        Vec<OrderPtr>,
    )
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        self.scan_order_batch(
            locked_order_batch,
            set_order_status,
            "computing asset contribution fractions",
            SolverOrderStatus::MathOverflow,
            |order_upread| {
                self.compute_asset_contribution_fractions_for_order(
                    total_asset_quantities,
                    order_quantities,
                    order_upread,
                )
            },
        )
    }

    fn solve_quotes_for<QuotePtr, UpRead, SetQuoteStatusFn, SetQuantityPossibleFn>(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        side: Side,
        locked_quotes: &mut Vec<(QuotePtr, UpRead)>,
        set_quote_status: SetQuoteStatusFn,
        set_quantity_possible: SetQuantityPossibleFn,
    ) -> Result<(Vec<QuotePtr>, Vec<QuotePtr>)>
    where
        QuotePtr: Clone,
        UpRead: Deref<Target = SolverQuote>,
        SetQuoteStatusFn: Fn(&mut UpRead, SolverQuoteStatus),
        SetQuantityPossibleFn: Fn(&mut UpRead, Amount),
    {
        let (baskets, _) =
            self.get_baskets_from_quotes(strategy_host, locked_quotes, &set_quote_status)?;

        let symbols: HashSet<_> = baskets
            .values()
            .map(|basket| {
                HashSet::from_iter(
                    basket
                        .basket_assets
                        .iter()
                        .map(|basket_asset| basket_asset.weight.asset.name.clone()),
                )
            })
            .concat();

        let symbols = symbols.into_iter().collect_vec();

        // We'll use price limit at current top of the book ±(buy|sell) price threshold
        let (asset_price_limits, _) = self.get_asset_price_limits(strategy_host, side, &symbols)?;

        // We'll calculate highest index prices within price limits
        let (index_price_limits, _) = self.get_index_price(&baskets, &asset_price_limits);

        let mut solved_quotes = Vec::new();
        let mut failed_quotes = Vec::new();

        for (quote_ptr, quote_upread) in locked_quotes {
            let collateral_amount = quote_upread.collateral_amount;

            if let Some(&index_price) = index_price_limits.get(&quote_upread.symbol) {
                let collateral_available = safe!(collateral_amount / self.fee_factor)
                    .ok_or_eyre("Fee factor multiplication error")?;

                let quantity_possible = safe!(collateral_available / index_price)
                    .ok_or_eyre("Index price division error")?;

                set_quantity_possible(quote_upread, quantity_possible);
                set_quote_status(quote_upread, SolverQuoteStatus::Ready);

                solved_quotes.push(quote_ptr.clone());
            } else {
                set_quote_status(quote_upread, SolverQuoteStatus::MissingPrices);
                failed_quotes.push(quote_ptr.clone());
            }
        }

        Ok((solved_quotes, failed_quotes))
    }

    fn solve_engagements_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        side: Side,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
    ) -> Result<(SimpleSolverEngagements, Vec<OrderPtr>)>
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        let (baskets, bad_index_symbol) =
            self.get_baskets(strategy_host, locked_order_batch, &set_order_status)?;

        let symbols: HashSet<_> = baskets
            .values()
            .map(|basket| {
                HashSet::from_iter(
                    basket
                        .basket_assets
                        .iter()
                        .map(|basket_asset| basket_asset.weight.asset.name.clone()),
                )
            })
            .concat();

        let symbols = symbols.into_iter().collect_vec();

        // We'll use price limit at current top of the book ±(buy|sell) price threshold
        let (asset_price_limits, bad_missing_asset_prices) =
            self.get_asset_price_limits(strategy_host, side, &symbols)?;

        if !bad_missing_asset_prices.is_empty() {
            // TBD: We abort if we have missing prices, but we could continue with remaining orders
            Err(eyre!(
                "Missing prices for assets: {}",
                bad_missing_asset_prices.into_iter().join(", ")
            ))?;
        }

        // We'll calculate highest index prices within price limits
        let (index_price_limits, bad_missing_index_prices) =
            self.get_index_price(&baskets, &asset_price_limits);

        if !bad_missing_index_prices.is_empty() {
            // TBD: We abort if we have missing prices, but we could continue with remaining orders
            Err(eyre!(
                "Missing prices for indexes: {}",
                bad_missing_index_prices.into_iter().join(", ")
            ))?;
        }

        // Next we want to know available liquidity for all the assets
        let asset_liquidity =
            strategy_host.get_liquidity(side.opposite_side(), &asset_price_limits)?;

        // Then we need to know how much quantity of each asset is needed for
        // each index order to fill up available collateral.
        //
        // TODO: These quantities will be used to compute contributions of each
        // index order, so perhaps we should limit available collateral here to
        // avoid huge orders overwhelming small orders. We could use hard limit,
        // or non-linear relationship between contribution and amount of collateral.
        // ^ this is to be explored
        //
        let (order_quantities, bad_compute_quantity) = self.compute_quantities_for(
            locked_order_batch,
            &set_order_status,
            &baskets,
            &index_price_limits,
        );

        // Next we sum up per asset quantities from all orders
        let total_asset_quantities = self.summarise_asset_quanties(&order_quantities)?;

        // Next we cap order quantity for each order to fit into liquidity
        let (order_contributions, bad_compute_contributions) = self.compute_contributions_for(
            locked_order_batch,
            &set_order_status,
            &asset_liquidity,
            &total_asset_quantities,
            &order_quantities,
        );

        // Then we recompute asset quantites based on capped order sizes
        let (order_asset_contributions, bad_compute_asset_contributions) = self
            .compute_asset_contributions_for(
                locked_order_batch,
                &set_order_status,
                &baskets,
                &order_contributions,
            );

        // Next we sum up per asset quantities from all orders
        let total_asset_quantities = self.summarise_asset_quanties(&order_asset_contributions)?;

        // Next we cap volley size of the whole batch
        let total_volley_size =
            self.compute_volley_size(&total_asset_quantities, &asset_price_limits)?;

        let (order_asset_contributions, bad_cap_volley_size) = self.cap_volley_size(
            locked_order_batch,
            &set_order_status,
            total_volley_size,
            order_asset_contributions,
        )?;

        // And we recomupte total asset quantites
        let total_asset_quantities = self.summarise_asset_quanties(&order_asset_contributions)?;

        // And now we compute contribution fraction for each asset, so that when
        // we get fills we can distribute those fills between index orders in
        // the batch proportionally to that fraction
        let (order_asset_contribution_fractions, bad_compute_asset_contribution_fractions) = self
            .compute_asset_contribution_fractions_for(
                locked_order_batch,
                &set_order_status,
                &total_asset_quantities,
                &order_asset_contributions,
            );

        // orders for which an error occurred
        let bad = [
            bad_index_symbol,
            bad_compute_quantity,
            bad_compute_contributions,
            bad_cap_volley_size,
            bad_compute_asset_contributions,
            bad_compute_asset_contribution_fractions,
        ]
        .concat();

        Ok((
            SimpleSolverEngagements {
                baskets,
                asset_price_limits,
                index_price_limits,
                quantity_contributions: order_asset_contributions,
                contribution_fractions: order_asset_contribution_fractions,
            },
            bad,
        ))
    }
}

impl SolverStrategy for SimpleSolver {
    fn query_collateral_management(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        order: Arc<RwLock<SolverOrder>>,
    ) -> Result<CollateralManagement> {
        let order = order.read();
        let collateral_amount = order.remaining_collateral;
        let index_symbol = &order.symbol;

        let basket = strategy_host
            .get_basket(index_symbol)
            .ok_or_eyre("Basket not found")?;

        let basket_assets = HashSet::<Symbol>::from_iter(
            basket
                .basket_assets
                .iter()
                .map(|basket_asset| basket_asset.weight.asset.name.clone()),
        );

        let symbols = basket_assets.into_iter().collect_vec();

        // Note: We use current price limits to understand how much collateral
        // will need to be moved to which sub-account. We understand that price
        // may move at any time, and that would affect the distribution of the
        // collateral too. We need to emiprically observe the impact of this
        // behaviour. Note that price limits already set higher requirement than
        // expected execution prices, so it is possible that price move won't
        // affect that much the execution of the index orders.
        let (prices, missing_symbols) =
            self.get_asset_price_limits(strategy_host, order.side, &symbols)?;

        if !missing_symbols.is_empty() {
            Err(eyre!("Missing symbols"))?;
        }

        // Note: We include collateral carried as it will contribute to total
        // quantity we will be able to buy or sell, however collateral manager
        // should check how much collateral is already on sub-accounts, and move
        // it accordingly to fulfill the requirements that we will return from
        // this function.
        let collateral_available =
            safe!(safe!(collateral_amount / self.fee_factor) + order.collateral_carried)
                .ok_or_eyre("Fee factor multiplication error")?;

        let index_price = basket.get_current_price(&prices)?;

        let mut collateral_management = CollateralManagement {
            chain_id: order.chain_id,
            address: order.address,
            client_order_id: order.client_order_id.clone(),
            side: order.side,
            collateral_amount,
            asset_requirements: HashMap::new(),
        };

        for basket_asset in &basket.basket_assets {
            let asset_symbol = &basket_asset.weight.asset.name;
            let asset_price = *prices.get(asset_symbol).ok_or_eyre("Missing asset price")?;

            // We calculate how big is the portion of the collateral that needs
            // to be assigned to this asset. This is critical when we route
            // collateral to sub-accounts, we must know how much to route and to
            // where.
            let asset_contribution = safe!(index_price / asset_price)
                .ok_or_eyre("Failed to compute asset contribution")?;

            let asset_requirement = safe!(collateral_available * asset_contribution)
                .ok_or_eyre("Failed to compute asset requirement")?;

            collateral_management
                .asset_requirements
                .insert(asset_symbol.clone(), asset_requirement);
            // ^ we could include whole asset instead of just symbol, because
            // perhaps we would have some additional information in the asset,
            // e.g. associated sub-accounts.
        }

        Ok(collateral_management)
    }

    fn solve_quotes(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        quote_requests: Vec<Arc<RwLock<SolverQuote>>>,
    ) -> Result<SolveQuotesResult> {
        let locked_quotes = quote_requests
            .iter()
            .map(|quote| (quote, quote.upgradable_read()));

        let (mut buys, mut sells): (Vec<_>, Vec<_>) =
            locked_quotes
                .into_iter()
                .partition_map(|(quote_ptr, quote_upread)| match quote_upread.side {
                    Side::Buy => Either::Left((quote_ptr, quote_upread)),
                    Side::Sell => Either::Right((quote_ptr, quote_upread)),
                });

        let set_quote_status = |upread: &mut RwLockUpgradableReadGuard<SolverQuote>, status| {
            upread.with_upgraded(|write| strategy_host.set_quote_status(write, status))
        };

        let set_quantity_possible = |upread: &mut RwLockUpgradableReadGuard<SolverQuote>,
                                     quantity_possible| {
            upread.with_upgraded(|write| write.quantity_possible = quantity_possible)
        };

        let (solved_buys, failed_buys) = self.solve_quotes_for(
            strategy_host,
            Side::Buy,
            &mut buys,
            set_quote_status,
            set_quantity_possible,
        )?;

        let (solved_sells, failed_sells) = self.solve_quotes_for(
            strategy_host,
            Side::Sell,
            &mut sells,
            set_quote_status,
            set_quantity_possible,
        )?;

        let result = SolveQuotesResult {
            solved_quotes: [solved_buys, solved_sells]
                .concat()
                .into_iter()
                .cloned()
                .collect_vec(),
            failed_quotes: [failed_buys, failed_sells]
                .concat()
                .into_iter()
                .cloned()
                .collect_vec(),
        };

        Ok(result)
    }

    fn solve_engagements(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        order_batch: Vec<Arc<RwLock<SolverOrder>>>,
    ) -> Result<SolveEngagementsResult> {
        let locked_order_batch = order_batch
            .iter()
            .map(|order| (order, order.upgradable_read()));

        let (mut buys, mut sells): (Vec<_>, Vec<_>) =
            locked_order_batch
                .into_iter()
                .partition_map(|(order_ptr, order_upread)| match order_upread.side {
                    Side::Buy => Either::Left((order_ptr, order_upread)),
                    Side::Sell => Either::Right((order_ptr, order_upread)),
                });

        let set_order_status = |upread: &mut RwLockUpgradableReadGuard<SolverOrder>, status| {
            upread.with_upgraded(|write| strategy_host.set_order_status(write, status))
        };

        let (mut engaged_buys, failed_buys) =
            self.solve_engagements_for(strategy_host, Side::Buy, &mut buys, set_order_status)?;

        let (engaged_sells, failed_sells) =
            self.solve_engagements_for(strategy_host, Side::Sell, &mut sells, set_order_status)?;

        if !engaged_sells.baskets.is_empty() {
            todo!("Selling isn't fully supported yet")
        }

        let mut engagenments = EngagedSolverOrders {
            batch_order_id: strategy_host.get_next_batch_order_id(),
            engaged_orders: Vec::new(),
        };

        for (order_ptr, order_upread) in buys {
            let address = &order_upread.address;
            let client_order_id = order_upread.client_order_id.clone();
            let address_client_order_id = (*address, client_order_id.clone());
            let index_symbol = &order_upread.symbol;
            let carried_collateral = order_upread.collateral_carried;

            let basket = engaged_buys
                .baskets
                .get(index_symbol)
                .ok_or_eyre("Missing basket")?;

            let index_price_limit = *engaged_buys
                .index_price_limits
                .get(index_symbol)
                .ok_or_eyre("Missing index price")?;

            let (order_quantity, quantity_contributions) = engaged_buys
                .quantity_contributions
                .remove(&address_client_order_id)
                .ok_or_eyre("Missing quantity contributions")?;

            let contribution_fractions = engaged_buys
                .contribution_fractions
                .remove(&address_client_order_id)
                .ok_or_eyre("Missing contribution fractions")?;

            let engaged_collateral =
                safe!(safe!(index_price_limit * order_quantity) * self.fee_factor)
                    .ok_or_eyre("Failed to compute enagaged collateral")?;

            let new_engaged_collateral = safe!(engaged_collateral - carried_collateral)
                .ok_or_eyre("Failed to compute new enagaged collateral")?;

            let engagement = SolverOrderEngagement {
                index_order: order_ptr.clone(),
                chain_id: order_upread.chain_id,
                address: order_upread.address,
                client_order_id: client_order_id.clone(),
                symbol: index_symbol.clone(),
                asset_contribution_fractions: contribution_fractions,
                asset_quantities: quantity_contributions,
                asset_price_limits: engaged_buys.asset_price_limits.clone(),
                basket: basket.clone(),
                engaged_quantity: order_quantity,
                engaged_price: index_price_limit,
                engaged_collateral,
                new_engaged_collateral,
                engaged_side: Side::Buy,
                filled_quantity: Amount::ZERO,
            };

            println!(
                "(simple-solver) Solver Order Engagement: {} {} eq={:0.5} ep={:0.5} ec={:0.5}",
                engagement.client_order_id,
                engagement.symbol,
                engagement.engaged_quantity,
                engagement.engaged_price,
                engagement.engaged_collateral
            );

            engagenments.engaged_orders.push(engagement);
        }

        let failed_buys = failed_buys.into_iter().cloned().collect_vec();
        let failed_sells = failed_sells.into_iter().cloned().collect_vec();

        Ok(SolveEngagementsResult {
            engaged_orders: engagenments,
            failed_orders: [failed_buys, failed_sells].concat(),
        })
    }
}

#[cfg(test)]
mod test {
    use chrono::{DateTime, Utc};
    use eyre::*;
    use parking_lot::RwLock;
    use rust_decimal::dec;
    use std::{
        cell::RefCell,
        collections::{hash_map::HashMap, VecDeque},
        sync::Arc,
    };

    use symm_core::{
        assert_decimal_approx_eq,
        core::{
            bits::*,
            test_util::{
                get_mock_address_1, get_mock_asset_1_arc, get_mock_asset_2_arc,
                get_mock_asset_3_arc, get_mock_asset_name_1, get_mock_asset_name_2,
                get_mock_asset_name_3, get_mock_index_name_1, get_mock_index_name_2,
            },
        },
        market_data::price_tracker::*,
    };

    use crate::{
        index::basket::*,
        solver::{
            solver::*,
            solver_order::{SolverOrder, SolverOrderStatus},
            solver_quote::{SolverQuote, SolverQuoteStatus},
        },
    };

    use test_case::test_case;

    use super::SimpleSolver;

    struct MockSolverStrategyHost {
        batch_order_ids: RefCell<VecDeque<BatchOrderId>>,
        baskets: HashMap<Symbol, Arc<Basket>>,
    }

    impl MockSolverStrategyHost {
        fn new() -> Self {
            Self {
                batch_order_ids: RefCell::new(VecDeque::new()),
                baskets: HashMap::new(),
            }
        }
    }

    impl SetSolverOrderStatus for MockSolverStrategyHost {
        fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus) {
            order.status = status;
        }

        fn set_quote_status(&self, quote: &mut SolverQuote, status: SolverQuoteStatus) {
            quote.status = status;
        }
    }

    impl SolverStrategyHost for MockSolverStrategyHost {
        fn get_basket(&self, symbol: &Symbol) -> Option<Arc<Basket>> {
            self.baskets.get(symbol).cloned()
        }

        fn get_liquidity(
            &self,
            side: Side,
            symbols: &HashMap<Symbol, Amount>,
        ) -> Result<HashMap<Symbol, Amount>> {
            let _ = symbols;
            let _ = side;
            Ok(HashMap::from([
                (get_mock_asset_name_1(), dec!(100.0)),
                (get_mock_asset_name_2(), dec!(200.0)),
                (get_mock_asset_name_3(), dec!(500.0)),
            ]))
        }

        fn get_next_batch_order_id(&self) -> BatchOrderId {
            self.batch_order_ids
                .borrow_mut()
                .pop_front()
                .expect("No more Batch Order IDs")
        }

        fn get_prices(&self, price_type: PriceType, symbols: &[Symbol]) -> GetPricesResponse {
            let _ = symbols;
            let _ = price_type;
            GetPricesResponse {
                prices: HashMap::from([
                    (get_mock_asset_name_1(), dec!(100.0)),
                    (get_mock_asset_name_2(), dec!(200.0)),
                    (get_mock_asset_name_3(), dec!(10.0)),
                ]),
                missing_symbols: Vec::new(),
            }
        }
    }

    fn make_basket_1() -> Arc<Basket> {
        Arc::new(Basket {
            basket_assets: vec![
                BasketAsset {
                    price: dec!(100.0),
                    quantity: dec!(8.0),
                    weight: AssetWeight {
                        asset: get_mock_asset_1_arc(),
                        weight: dec!(0.8),
                    },
                },
                BasketAsset {
                    price: dec!(200.0),
                    quantity: dec!(1.0),
                    weight: AssetWeight {
                        asset: get_mock_asset_2_arc(),
                        weight: dec!(0.2),
                    },
                },
            ],
            target_price: dec!(1000.0),
        })
    }

    fn make_basket_2() -> Arc<Basket> {
        Arc::new(Basket {
            basket_assets: vec![
                BasketAsset {
                    price: dec!(100.0),
                    quantity: dec!(5.0),
                    weight: AssetWeight {
                        asset: get_mock_asset_1_arc(),
                        weight: dec!(0.5),
                    },
                },
                BasketAsset {
                    price: dec!(10.0),
                    quantity: dec!(50.0),
                    weight: AssetWeight {
                        asset: get_mock_asset_3_arc(),
                        weight: dec!(0.5),
                    },
                },
            ],
            target_price: dec!(1000.0),
        })
    }

    fn gen_client_order_id(index: u32) -> ClientOrderId {
        ClientOrderId::from(format!("C-{:02}", index))
    }

    fn gen_client_quote_id(index: u32) -> ClientQuoteId {
        ClientQuoteId::from(format!("Q-{:02}", index))
    }

    fn make_solver_order(
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Arc<RwLock<SolverOrder>> {
        Arc::new(RwLock::new(SolverOrder {
            chain_id,
            address,
            client_order_id,
            symbol,
            side,
            remaining_collateral: collateral_amount,
            payment_id: None,
            engaged_collateral: Amount::ZERO,
            collateral_carried: Amount::ZERO,
            collateral_spent: Amount::ZERO,
            filled_quantity: Amount::ZERO,
            timestamp,
            status: SolverOrderStatus::Open,
            lots: Vec::new(),
        }))
    }

    fn make_solver_quote(
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Arc<RwLock<SolverQuote>> {
        Arc::new(RwLock::new(SolverQuote {
            chain_id,
            address,
            client_quote_id,
            symbol,
            side,
            collateral_amount,
            quantity_possible: Amount::ZERO,
            timestamp,
            status: SolverQuoteStatus::Open,
        }))
    }

    /// Simple Solver - A simple implementation of the SolverStrategy
    /// ------
    /// Solver Strategy is a plugin for Solver, the role of which is to compute
    /// batches of Asset Orders that are then sent to exchange. An individual
    /// Index Order is an order from the user to buy or sell an Index. And Index
    /// is a basket of assets in pre-defined quantities. A quantity of an Index
    /// is a multiplication factor by which those pre-defined quantities need to
    /// be multiplied to cover for an Index Order in that quantity. The
    /// individual asset quantities resulting from that multiplication are then
    /// placed in the Asset Orders, which are sent to exchange.
    ///
    /// Due to order-rate limitations as well as minimum-order-size, it would be
    /// unfeasable to send all those asset orders for each Index Order
    /// individually.  This is why we take a batch of Index Order (multiple
    /// orders), and we compute gross quantity of each asset that we need to
    /// order to satisfy all those Index Orders and not just one.
    ///
    /// When constructing this coallesced Asset Orders, we must adhere to additional
    /// limits set by business logic:
    /// - Max Order Volley Size - maximum amount of collateral that single Index
    /// Order can use in one batch (rest needs to be carried over to next batch)
    /// - Max Batch Vollet Size - maximum amount of collateral that all Index
    /// Orders in the batch altogether can use
    ///
    /// Additionally not all of the collateral available for Index Order can be
    /// used in Asset Orders. We must account for potential fees, so we use
    /// - Fee Factor - 1.0 + maximum fee per order we would expect to pay
    ///
    /// When sending orders to exchange we must use Limit orders to ensure we
    /// have a price limit that won't be exceeded, so that we can guarantee max
    /// volley sizes won't be exceeded. We use deviation from top of the book
    /// price:
    /// - Price Threshold - maximum deviation of price from top of the book for
    /// this batch order
    ///
    /// Note that Index Orders in this batch will be carried over to next batch
    /// is not all collateral was utilised, and in that next batch different
    /// price limits will be used following the market as it moves.
    ///
    /// SimpleSolver fits Asset Orders into liquidity available within price
    /// threshold. This means that narrower price threshold may make batch
    /// smaller.
    ///
    /// Fairness of the SimpleSolver is based on the quantity on Index Order
    /// clamped by max order volley size. The purpose of max order volley size
    /// is to improve fairness, as otherwise bigger Index Orders would outweigh
    /// smaller ones.
    ///
    fn _description() {}

    #[test_case(
        "Unlimited",
        (dec!(0.0), dec!(1.0), dec!(1_000_000.0), dec!(1_000_000.0)),
        vec![dec!(1000.0), dec!(5000.0)],
        vec![dec!(1.0), dec!(5.0)],
        vec![
            (dec!(1.0), dec!(1000.0), dec!(1000.0)),
            (dec!(5.0), dec!(1000.0), dec!(5000.0))
        ]; "unlimited"
    )]
    #[test_case(
        "Max Order Volley Set",
        (dec!(0.0), dec!(1.0), dec!(1_000.0), dec!(1_000_000.0)),
        vec![dec!(1000.0), dec!(5000.0)],
        vec![dec!(1.0), dec!(5.0)],
        vec![
            (dec!(1.0), dec!(1000.0), dec!(1000.0)),
            (dec!(1.0), dec!(1000.0), dec!(1000.0))
        ]; "max_order_volley_set"
    )]
    #[test_case(
        "Max Batch Volley Set",
        (dec!(0.0), dec!(1.0), dec!(1_000_000.0), dec!(1_000.0)),
        vec![dec!(1000.0), dec!(5000.0)],
        vec![dec!(1.0), dec!(5.0)],
        vec![
            (dec!(0.16666), dec!(1000.0), dec!(166.66666)),
            (dec!(0.83333), dec!(1000.0), dec!(833.33333))
        ]; "max_batch_volley_set"
    )]
    #[test_case(
        "Max Volleys Set",
        (dec!(0.0), dec!(1.0), dec!(1_000.0), dec!(1_500.0)),
        vec![dec!(1000.0), dec!(5000.0)],
        vec![dec!(1.0), dec!(5.0)],
        vec![
            (dec!(0.75), dec!(1000.0), dec!(750.0)),
            (dec!(0.75), dec!(1000.0), dec!(750.0))
        ]; "max_volleys_set"
    )]
    #[test_case(
        "Fee Factor 1%",
        (dec!(0.0), dec!(1.01), dec!(1_000_000.0), dec!(1_000_000.0)),
        vec![dec!(1000.0), dec!(5000.0)],
        vec![dec!(0.990099), dec!(4.950495)],
        vec![
            (dec!(0.99009), dec!(1000.0), dec!(1000.0)),
            (dec!(4.95049), dec!(1000.0), dec!(5000.0))
        ]; "fee_factor_1pct"
    )]
    #[test_case(
        "Price Threshold 1%",
        (dec!(0.01), dec!(1.0), dec!(1_000_000.0), dec!(1_000_000.0)),
        vec![dec!(1000.0), dec!(5000.0)],
        vec![dec!(0.990099), dec!(4.950495)],
        vec![
            (dec!(0.99009), dec!(1010.0), dec!(1000.0)),
            (dec!(4.95049), dec!(1010.0), dec!(5000.0))
        ]; "price_threshold_1pct"
    )]
    fn test_simple_solver(
        title: &str,
        params: (Amount, Amount, Amount, Amount),
        collateral_amount: Vec<Amount>,
        expected_quotes: Vec<Amount>,
        expected: Vec<(Amount, Amount, Amount)>,
    ) {
        println!("Test Case: {}", title);

        let timestamp = Utc::now();

        let mut strategy_host = MockSolverStrategyHost::new();

        strategy_host
            .batch_order_ids
            .borrow_mut()
            .push_back("B-01".into());

        strategy_host
            .baskets
            .insert(get_mock_index_name_1(), make_basket_1());

        strategy_host
            .baskets
            .insert(get_mock_index_name_2(), make_basket_2());

        assert_eq!(collateral_amount.len(), 2);

        let quote_requests = vec![
            make_solver_quote(
                1,
                get_mock_address_1(),
                gen_client_quote_id(1),
                get_mock_index_name_1(),
                Side::Buy,
                collateral_amount[0],
                timestamp,
            ),
            make_solver_quote(
                2,
                get_mock_address_1(),
                gen_client_quote_id(2),
                get_mock_index_name_2(),
                Side::Buy,
                collateral_amount[1],
                timestamp,
            ),
        ];

        let order_batch = vec![
            make_solver_order(
                1,
                get_mock_address_1(),
                gen_client_order_id(1),
                get_mock_index_name_1(),
                Side::Buy,
                collateral_amount[0],
                timestamp,
            ),
            make_solver_order(
                2,
                get_mock_address_1(),
                gen_client_order_id(2),
                get_mock_index_name_2(),
                Side::Buy,
                collateral_amount[1],
                timestamp,
            ),
        ];

        let simple_solver = SimpleSolver::new(params.0, params.1, params.2, params.3);

        let solved_quotes = simple_solver
            .solve_quotes(&strategy_host, quote_requests)
            .expect("Failed to solve quotes");

        assert!(solved_quotes.failed_quotes.is_empty());

        for n in 0..expected_quotes.len() {
            let quote_read = solved_quotes.solved_quotes[n].read();
            println!(
                "for {} {} <> {}",
                n, quote_read.quantity_possible, expected_quotes[n]
            );
            assert!(matches!(quote_read.status, SolverQuoteStatus::Ready));
            assert_decimal_approx_eq!(
                quote_read.quantity_possible,
                expected_quotes[n],
                dec!(0.00001)
            );
        }

        let batch = simple_solver
            .solve_engagements(&strategy_host, order_batch)
            .expect("Failed to solve engagements");

        assert_eq!(batch.engaged_orders.engaged_orders.len(), expected.len());
        for n in 0..expected.len() {
            assert_decimal_approx_eq!(
                batch.engaged_orders.engaged_orders[n].engaged_quantity,
                expected[n].0,
                dec!(0.00001)
            );
            assert_decimal_approx_eq!(
                batch.engaged_orders.engaged_orders[n].engaged_price,
                expected[n].1,
                dec!(0.00001)
            );
            assert_decimal_approx_eq!(
                batch.engaged_orders.engaged_orders[n].engaged_collateral,
                expected[n].2,
                dec!(0.00001)
            );
        }
    }
}
