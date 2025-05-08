use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    ops::Deref,
    sync::Arc,
};

use eyre::{eyre, OptionExt, Result};
use itertools::{Either, Itertools};
use parking_lot::RwLock;
use safe_math::safe;

use crate::{
    core::{
        bits::{Amount, ClientOrderId, PriceType, Side, Symbol},
        decimal_ext::DecimalExt,
    },
    index::basket::Basket,
    solver::solver::{
        EngagedSolverOrders, SolverOrder, SolverOrderEngagement, SolverOrderStatus, SolverStrategy,
        SolverStrategyHost,
    },
};

struct SimpleSolverEngagements {
    baskets: HashMap<Symbol, Arc<Basket>>,
    asset_price_limits: HashMap<Symbol, Amount>,
    index_price_limits: HashMap<Symbol, Amount>,
    quantity_contributions: HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
    contribution_fractions: HashMap<ClientOrderId, HashMap<Symbol, Amount>>,
}

pub struct SimpleSolver {
    /// When testing for liquidity, this sets the limit as to how far from top
    /// of the book we want to go, e.g. 0.01 (max price move = 1%)
    price_threshold: Amount,
    /// When calculating quantity that collateral can buy, we need to factor in
    /// fees, e.g. 1.001 (max fee = 0.1%)
    fee_factor: Amount,
}

impl SimpleSolver {
    pub fn new(price_threshold: Amount, fee_factor: Amount) -> Self {
        Self {
            price_threshold,
            fee_factor,
        }
    }

    /// Scan order batch for baskets
    ///
    fn get_baskets<OrderPtr, UpRead, SetOrderStatusFn>(
        &mut self,
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
                match strategy_host
                    .get_basket_manager()
                    .read()
                    .get_basket(&symbol)
                    .cloned()
                {
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
        let get_prices = strategy_host
            .get_price_tracker()
            .read()
            .get_prices(price_type, &symbols);

        let prices_len = get_prices.prices.len();

        for (k, v) in &get_prices.prices {
            println!("Price: {:?} {} {}", side, k, v);
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
            println!("Price Limit: {} {}", k, v);
        }

        Ok((price_limits, get_prices.missing_symbols))
    }

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
                    println!("Index Price: {} {}", symbol, price);
                    Either::Left((symbol, price))
                }
                Err(err) => {
                    eprintln!("Failed to compute index price for {}: {:?}", symbol, err);
                    Either::Right(symbol)
                }
            });

        (HashMap::from_iter(index_prices.into_iter()), bad)
    }

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

        let collateral_available = safe!(collateral_amount / self.fee_factor)
            .ok_or_eyre("Fee factor multiplication error")?;

        let index_order_quantity = safe!(collateral_available / index_price)
            .ok_or_eyre("Index order quantity computation error")?;

        println!(
            "Collateral to Quantity for Index Order: {} c={:0.5} ca={:0.5} p={:0.5} q={:0.5} ff={:0.5}",
            client_order_id, collateral_amount, collateral_available, index_price, index_order_quantity, self.fee_factor
        );

        let asset_quantities: HashMap<_, _> = basket
            .basket_assets
            .iter()
            .map_while(|basket_asset| {
                let asset_symbol = &basket_asset.weight.asset.name;
                let asset_quantity = safe!(basket_asset.quantity * index_order_quantity)?;
                println!(
                    "Asset Quantity for Index Order: {} {} q={:0.5} baq={:0.5}",
                    client_order_id, asset_symbol, asset_quantity, basket_asset.quantity
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

    fn compute_quantities_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        index_price_limits: &HashMap<Symbol, Amount>,
    ) -> (
        HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
        Vec<OrderPtr>,
    )
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        let mut bad_compute_quantity = Vec::new();
        let mut order_quantities = HashMap::new();

        locked_order_batch.retain_mut(|(order_ptr, order_upread)| {
            match (|| self.compute_quantities_for_order(baskets, index_price_limits, order_upread))(
            ) {
                Ok(res) => {
                    order_quantities.insert(order_upread.client_order_id.clone(), res);
                    true
                }
                Err(err) => {
                    eprintln!("Error while computing quantities: {:?}", err);
                    set_order_status(order_upread, SolverOrderStatus::MathOverflow);
                    bad_compute_quantity.push(order_ptr.clone());
                    false
                }
            }
        });

        (order_quantities, bad_compute_quantity)
    }

    fn summarise_asset_quanties(
        &self,
        order_quantities: &HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
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

    fn compute_contribution_for_order<UpRead>(
        &self,
        total_asset_liquidity: &HashMap<Symbol, Amount>,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
        order_upread: &UpRead,
    ) -> Result<Amount>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let client_order_id = &order_upread.client_order_id;

        let (order_quantity, asset_quanties) = order_quantities
            .get(&client_order_id)
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
                "Fitting Quantity for Index Order: {} {} {:0.5} tal={:0.5} taq={:0.5} acf={:0.5} alc={:0.5} poq={:0.5}",
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

    fn compute_contributions_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        total_asset_liquidity: &HashMap<Symbol, Amount>,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
    ) -> (HashMap<ClientOrderId, Amount>, Vec<OrderPtr>)
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        let mut bad_compute_contributions = Vec::new();
        let mut order_contributions = HashMap::new();

        locked_order_batch.retain_mut(|(order_ptr, order_upread)| {
            match (|| {
                self.compute_contribution_for_order(
                    total_asset_liquidity,
                    total_asset_quantities,
                    order_quantities,
                    order_upread,
                )
            })() {
                Ok(res) => {
                    order_contributions.insert(order_upread.client_order_id.clone(), res);
                    true
                }
                Err(err) => {
                    eprintln!("Error while computing contributions: {:?}", err);
                    set_order_status(order_upread, SolverOrderStatus::MathOverflow);
                    bad_compute_contributions.push(order_ptr.clone());
                    false
                }
            }
        });

        (order_contributions, bad_compute_contributions)
    }

    fn compute_asset_contributions_for_order<UpRead>(
        &self,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        order_contributions: &HashMap<ClientOrderId, Amount>,
        order_upread: &UpRead,
    ) -> Result<(Amount, HashMap<Symbol, Amount>)>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let client_order_id = &order_upread.client_order_id;
        let index_symbol = &order_upread.symbol;

        let mut asset_contributions = HashMap::new();

        let order_quantity = *order_contributions
            .get(&client_order_id)
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

    fn compute_asset_contributions_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        baskets: &HashMap<Symbol, Arc<Basket>>,
        order_contributions: &HashMap<ClientOrderId, Amount>,
    ) -> (
        HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
        Vec<OrderPtr>,
    )
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        let mut bad_compute_asset_contributions = Vec::new();
        let mut order_asset_contributions = HashMap::new();

        locked_order_batch.retain_mut(|(order_ptr, order_upread)| {
            match (|| {
                self.compute_asset_contributions_for_order(
                    baskets,
                    order_contributions,
                    order_upread,
                )
            })() {
                Ok(res) => {
                    order_asset_contributions.insert(order_upread.client_order_id.clone(), res);
                    true
                }
                Err(err) => {
                    eprintln!("Error while computing asset contributions: {:?}", err);
                    set_order_status(order_upread, SolverOrderStatus::MathOverflow);
                    bad_compute_asset_contributions.push(order_ptr.clone());
                    false
                }
            }
        });

        (order_asset_contributions, bad_compute_asset_contributions)
    }

    fn compute_asset_contribution_fractions_for_order<UpRead>(
        &self,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
        order_upread: &UpRead,
    ) -> Result<HashMap<Symbol, Amount>>
    where
        UpRead: Deref<Target = SolverOrder>,
    {
        let client_order_id = &order_upread.client_order_id;

        let (_, asset_quanties) = order_quantities
            .get(&client_order_id)
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
                "Asset Fractions for Index Order: {} {} taq={:0.5} acf={:0.5}",
                client_order_id, asset_symbol, total_asset_quantity, asset_contribution_fraction,
            );
        }

        Ok(asset_contribution_fractions)
    }

    fn compute_asset_contribution_fractions_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &self,
        locked_order_batch: &mut Vec<(OrderPtr, UpRead)>,
        set_order_status: SetOrderStatusFn,
        total_asset_quantities: &HashMap<Symbol, Amount>,
        order_quantities: &HashMap<ClientOrderId, (Amount, HashMap<Symbol, Amount>)>,
    ) -> (
        HashMap<ClientOrderId, HashMap<Symbol, Amount>>,
        Vec<OrderPtr>,
    )
    where
        OrderPtr: Clone,
        UpRead: Deref<Target = SolverOrder>,
        SetOrderStatusFn: Fn(&mut UpRead, SolverOrderStatus),
    {
        let mut bad_compute_contributions = Vec::new();
        let mut order_contributions = HashMap::new();

        locked_order_batch.retain_mut(|(order_ptr, order_upread)| {
            match (|| {
                self.compute_asset_contribution_fractions_for_order(
                    total_asset_quantities,
                    order_quantities,
                    order_upread,
                )
            })() {
                Ok(res) => {
                    order_contributions.insert(order_upread.client_order_id.clone(), res);
                    true
                }
                Err(err) => {
                    eprintln!(
                        "Error while computing asset contribution fractions: {:?}",
                        err
                    );
                    set_order_status(order_upread, SolverOrderStatus::MathOverflow);
                    bad_compute_contributions.push(order_ptr.clone());
                    false
                }
            }
        });

        (order_contributions, bad_compute_contributions)
    }

    fn solve_engagements_for<OrderPtr, UpRead, SetOrderStatusFn>(
        &mut self,
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
        let asset_liquidity = strategy_host
            .get_book_manager()
            .read()
            .get_liquidity(side.opposite_side(), &asset_price_limits)?;

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

        // And now we compute contribution fraction for each asset, so that when
        // we get fills we can distribute those fills between index orders in
        // the batch proportionally to that fraction
        let (order_asset_contribution_fractions, bad_compute_asset_contribution_fractions) = self
            .compute_asset_contribution_fractions_for(
                locked_order_batch,
                &set_order_status,
                &total_asset_quantities,
                &order_quantities,
            );

        // orders for which an error occurred
        let bad = [
            bad_index_symbol,
            bad_compute_quantity,
            bad_compute_contributions,
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
    fn solve_engagements(
        &mut self,
        strategy_host: &dyn SolverStrategyHost,
    ) -> Result<Option<EngagedSolverOrders>> {
        let order_batch = strategy_host.get_order_batch();

        if order_batch.is_empty() {
            // it is not an error if nothing there to process
            return Ok(None);
        }

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

        let (mut engaged_buys, failed_buys) =
            self.solve_engagements_for(strategy_host, Side::Buy, &mut buys, |upread, status| {
                upread.with_upgraded(|write| strategy_host.set_order_status(write, status))
            })?;

        let (engaged_sells, failed_sells) =
            self.solve_engagements_for(strategy_host, Side::Sell, &mut sells, |upread, status| {
                upread.with_upgraded(|write| strategy_host.set_order_status(write, status))
            })?;

        if !failed_buys.is_empty() || !failed_sells.is_empty() {
            // TBD: We could ignore those, and continue with the ones that were
            // good.  Also some errors are temporary if, e.g. market data was
            // missing, so we should retry later.
            Err(eyre!("Failed to solve engagements for some of the orders"))?;
        }

        if !engaged_sells.baskets.is_empty() {
            todo!("Selling isn't fully supported yet")
        }

        let mut engagenments = EngagedSolverOrders {
            batch_order_id: strategy_host.get_next_batch_order_id(),
            engaged_orders: Vec::new(),
        };

        for (order_ptr, order_upread) in buys {
            let client_order_id = order_upread.client_order_id.clone();
            let index_symbol = &order_upread.symbol;

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
                .remove(&client_order_id)
                .ok_or_eyre("Missing quantity contributions")?;

            let contribution_fractions = engaged_buys
                .contribution_fractions
                .remove(&client_order_id)
                .ok_or_eyre("Missing contribution fractions")?;

            let engaged_collateral =
                safe!(safe!(index_price_limit * order_quantity) * self.fee_factor)
                    .ok_or_eyre("Failed to compute enagaged collateral")?;

            let engagement = SolverOrderEngagement {
                index_order: order_ptr.clone(),
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
                engaged_side: Side::Buy,
                filled_quantity: Amount::ZERO,
            };

            println!(
                "Solver Order Engagement: {} {} eq={:0.5} ep={:0.5} ec={:0.5}",
                engagement.client_order_id,
                engagement.symbol,
                engagement.engaged_quantity,
                engagement.engaged_price,
                engagement.engaged_collateral
            );

            engagenments.engaged_orders.push(RwLock::new(engagement));
        }

        Ok(Some(engagenments))
    }
}
