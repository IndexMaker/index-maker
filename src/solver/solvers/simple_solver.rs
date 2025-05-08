use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::Arc,
};

use itertools::{partition, Itertools};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use safe_math::safe;

use crate::{
    core::{
        bits::{Amount, BatchOrderId, ClientOrderId, PriceType, Symbol},
        decimal_ext::DecimalExt,
    },
    index::{basket::Basket, basket_manager::BasketManager},
    market_data::{order_book::order_book_manager::OrderBookManager, price_tracker::PriceTracker},
    solver::solver::{
        EngageOrder, EngagedOrders, FindOrderContribution, SolverOrder, SolverOrderStatus,
        SolverStrategy, SolverStrategyHost,
    },
};

/// Simplest possible solver for computing a set of rebalancing orders.
struct MoreOrders<'a> {
    locked_orders: Vec<(
        &'a Arc<RwLock<SolverOrder>>,
        RwLockUpgradableReadGuard<'a, SolverOrder>,
    )>,
    symbols: Vec<Symbol>,
    baskets: HashMap<Symbol, Arc<Basket>>,
    index_prices: HashMap<Symbol, Amount>,
    asset_prices: HashMap<Symbol, Amount>,
}

struct FindOrderLiquidity {
    /// We sum across all Index Orders the quantity of each asset multiplied by
    /// order quantity
    asset_total_order_quantity: HashMap<Symbol, Amount>,
    /// We calculate weighted average of liquidity across all Index Orders
    asset_total_weighted_liquidity: HashMap<Symbol, Amount>,
}

pub struct SimpleSolver {}

impl SimpleSolver {
    pub fn new() -> Self {
        Self {}
    }

    fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus) {
        println!(
            "Set Index Order Status: {} {:?}",
            order.client_order_id, status
        );
        order.status = status;
    }

    fn more_orders<'a>(
        &self,
        index_orders: &'a Vec<Arc<RwLock<SolverOrder>>>,
        strategy_host: &dyn SolverStrategyHost,
    ) -> MoreOrders<'a> {
        println!("\nMore Orders...");
        // Lock all Index Orders in the batch - for reading with intention to write
        let mut locked_orders = index_orders
            .iter()
            .map(|order| (order, order.upgradable_read()))
            .collect_vec();

        let basket_manager = strategy_host.get_basket_manager().read();
        let mut symbols = HashSet::new();
        let mut baskets = HashMap::new();

        // Collect baskets across all Index Orders in this batch
        let partition_point = partition(&mut locked_orders, |(_, order)| {
            match baskets.entry(order.symbol.clone()) {
                Entry::Occupied(_) => true,
                Entry::Vacant(entry) => match basket_manager.get_basket(&order.symbol) {
                    Some(basket) => {
                        entry.insert(basket.clone());
                        true
                    }
                    None => false,
                },
            }
        });

        // Unlocks and remove Index Orders with wrong index symbol
        locked_orders
            .splice(partition_point.., [])
            .for_each(|(_, mut order)| {
                order.with_upgraded(|order| {
                    self.set_order_status(order, SolverOrderStatus::InvalidSymbol);
                });
            });

        // Collect symbols across all baskets in the batch
        baskets.iter().for_each(|(_, basket)| {
            basket.basket_assets.iter().for_each(|asset| {
                symbols.insert(asset.weight.asset.name.clone());
            });
        });

        // We don't need set anymore... convert into vector
        let symbols = symbols.iter().cloned().collect_vec();

        // Get prices for all the assets in this batch of index orders
        let individual_asset_prices = strategy_host
            .get_price_tracker()
            .read()
            .get_prices(PriceType::VolumeWeighted, &symbols);

        if !individual_asset_prices.missing_symbols.is_empty() {
            // Note: We just log the fact, and we will mark corresponding
            // index orders as failed with missing prices
            println!(
                "Some assets have missing prices: {}",
                individual_asset_prices.missing_symbols.iter().join(", ")
            );
        }

        individual_asset_prices
            .prices
            .iter()
            .for_each(|(a, p)| println!(" * individual_asset_prices > ({:5} p={:0.5})", a, p));

        // Collect prices of all indexes in the batch
        let mut individual_index_prices = HashMap::new();
        let mut missing_index_prices = HashSet::new();

        // ...we drop baskets if we cannot have price for it
        baskets.retain(|index_symbol, basket| {
            match basket.get_current_price(&individual_asset_prices.prices) {
                Ok(price) => {
                    individual_index_prices.insert(index_symbol.clone(), price);
                    true
                }
                Err(_) => {
                    missing_index_prices.insert(index_symbol.clone());
                    false
                }
            }
        });

        // ...we drop index orders if we cannot get the price for index
        if !missing_index_prices.is_empty() {
            locked_orders.retain_mut(|(_, order)| {
                if missing_index_prices.contains(&order.symbol) {
                    order.with_upgraded(|order| {
                        // TODO: An order with missing prices can be reinserted, when
                        // new prices are available, and removed if too long in queue.
                        self.set_order_status(order, SolverOrderStatus::MissingPrices);
                    });
                    false
                } else {
                    true
                }
            });
        };

        MoreOrders {
            locked_orders,
            symbols,
            baskets,
            index_prices: individual_index_prices,
            asset_prices: individual_asset_prices.prices,
        }
    }

    fn find_order_liquidity(
        &self,
        more_orders: &mut MoreOrders,
        strategy_host: &dyn SolverStrategyHost,
    ) -> FindOrderLiquidity {
        println!("\nFind Order Liquidity...");
        println!("   c   collateral");
        println!("   q   quantity");
        println!("   l   liquidity");
        println!("");
        let mut asset_total_order_quantity = HashMap::new();
        let mut asset_total_weighted_liquidity = HashMap::new();
        let threshold = Amount::new(1, 2); // TODO: Should not hardcode

        let order_book_manager = strategy_host.get_book_manager().read();
        more_orders.locked_orders.retain_mut(|(_, index_order)| {
            let result = Some(&index_order).and_then(|update| {
                let side = update.side;
                let order_collateral_amount = update.remaining_collateral;

                // We should be able to get basket and its current price, as in locked_orders
                // we only keep orders that weren't erroneous
                let current_price = more_orders
                    .index_prices
                    .get(&index_order.symbol)
                    .expect("Missing index price");

                let basket = more_orders
                    .baskets
                    .get(&index_order.symbol)
                    .expect("Missing basket");

                let fee_factor = safe!(Amount::ONE + Amount::new(1, 3))?; // TODO: shouldn't be hardcoded
                let order_quantity =
                    safe! {order_collateral_amount / safe!(*current_price * fee_factor)?}?;

                println!(
                    " * index order: {:5} {} < {} {:?} c={:0.5} q={:0.5}",
                    index_order.symbol,
                    index_order.original_client_order_id,
                    update.client_order_id,
                    side,
                    order_collateral_amount,
                    order_quantity,
                );

                println!("\n- Find target asset prices and quantites...\n");
                let target_asset_prices_and_quantites = basket
                    .basket_assets
                    .iter()
                    .map_while(|asset| {
                        let asset_symbol = asset.weight.asset.name.clone();
                        let asset_price = more_orders
                            .asset_prices
                            .get(&asset_symbol)
                            .expect("Missing asset price");
                        //
                        // Formula:
                        //      quantity of asset to order = quantity of index order
                        //                                 * quantity of asset in basket
                        //
                        let asset_quantity = safe!(asset.quantity * order_quantity)?;

                        println!(
                            " * asset_quantity {:5} {:0.5} = {:0.5} * {:0.5}",
                            asset_symbol, asset_quantity, asset.quantity, order_quantity
                        );

                        println!(" * asset_price {:5} {:0.5}", asset_symbol, asset_price);

                        match asset_total_order_quantity.entry(asset.weight.asset.name.clone()) {
                            Entry::Occupied(mut entry) => {
                                let x: &Amount = entry.get();
                                entry.insert(safe!(*x + asset_quantity)?);
                            }
                            Entry::Vacant(entry) => {
                                entry.insert(asset_quantity);
                            }
                        }

                        Some((asset_symbol, *asset_price, asset_quantity))
                    })
                    .collect_vec();

                // There was some error - math overflow probably, we'll drop that order
                if target_asset_prices_and_quantites.len() < basket.basket_assets.len() {
                    return None;
                }

                // Split out quantities { asset_symbol => (price, quantity) } into { asset_symbol => quantity }
                let target_asset_quantites = HashMap::<Symbol, Amount>::from_iter(
                    target_asset_prices_and_quantites
                        .iter()
                        .map(|(k, _, q)| (k.clone(), *q)),
                );

                // Split out prices { asset_symbol => (price, quantity) } into { asset_symbol => price }
                let target_asset_prices = HashMap::from_iter(
                    target_asset_prices_and_quantites
                        .iter()
                        .map(|(k, p, _)| (k.clone(), *p)),
                );

                println!("\n- Find liquidity for prices with threshold...\n");

                let liquidity = order_book_manager
                    .get_liquidity(side.opposite_side(), &target_asset_prices, threshold)
                    .ok()?;

                for (asset_symbol, asset_liquidity) in liquidity {
                    println!(
                        " * liquidity >> {:5} t={:0.5} l={:0.5}",
                        asset_symbol, threshold, asset_liquidity
                    );
                    //
                    // We're collecting per asset sums, so that we will be able
                    // to calculate weighted average for asset liquidity. It is weighted
                    // proportionally to asset contribution in the whole index order batch.
                    //
                    // Formula:
                    //      asset quantity = quantity of asset in basket
                    //                     * quantity of index order
                    //
                    //      asset liquidity = quantity of asset in basket
                    //                      * quantity of index order
                    //                      * asset liquidity at price threshold
                    //
                    let asset_quantity = target_asset_quantites.get(&asset_symbol)?;
                    let asset_liquidity = safe!(*asset_quantity * asset_liquidity)?;

                    println!(
                        " * asset_total_weighted_liquidity << {:5} l={:0.5} q={:0.5}\n",
                        asset_symbol, asset_liquidity, *asset_quantity
                    );

                    match asset_total_weighted_liquidity.entry(asset_symbol.clone()) {
                        Entry::Occupied(mut entry) => {
                            let (weighted_sum, total_weight): &(Amount, Amount) = entry.get();
                            entry.insert((
                                safe!(*weighted_sum + asset_liquidity)?,
                                safe!(*total_weight + *asset_quantity)?,
                            ));
                        }
                        Entry::Vacant(entry) => {
                            entry.insert((asset_liquidity, *asset_quantity));
                        }
                    }
                }
                Some(())
            });
            if let None = result {
                index_order.with_upgraded(|order| {
                    self.set_order_status(order, SolverOrderStatus::MathOverflow)
                });
                false
            } else {
                true
            }
        });

        FindOrderLiquidity {
            asset_total_order_quantity,
            asset_total_weighted_liquidity: asset_total_weighted_liquidity
                .into_iter()
                .filter_map(|(k, (w, s))| safe!(w / s).map(|x| (k, x)))
                .collect(),
        }
    }

    fn find_order_contribution(
        &self,
        more_orders: &mut MoreOrders,
        order_liquidity: FindOrderLiquidity,
    ) -> HashMap<ClientOrderId, FindOrderContribution> {
        println!("\nFind Order Contribution...");
        println!("   q   asset_order_quantity");
        println!("   tq  asset_total_quantity");
        println!("   tl  asset_liquidity");
        println!("   acf asset_contribution_fraction");
        println!("   alc asset_liquidity_contribution");
        println!("   of  order_fraction");
        println!("   oq  order_quantity");
        println!("");
        let mut all_contributions = HashMap::new();

        // Remove erroneous orders
        more_orders.locked_orders.retain_mut(|(_, index_order)| {
            let basket = more_orders.baskets.get(&index_order.symbol);

            // Find error
            let result = Some(&index_order).and_then(|update| {
                let basket = basket?;
                let order_quantity = update.remaining_collateral;
                let mut contribution = FindOrderContribution {
                    asset_contribution_fraction: HashMap::new(),
                    asset_liquidity_contribution: HashMap::new(),
                    order_fraction: Amount::ONE,
                    order_quantity: Amount::ZERO,
                };

                let count = basket
                    .basket_assets
                    .iter()
                    .map_while(|asset| {
                        // Formula:
                        //      quantity of asset in basket for index order = quantity of asset in basket
                        //                                                  * quantity of index order
                        let asset_order_quantity = safe!(asset.quantity * order_quantity)?;

                        // Total quantity of asset across all index orders in batch
                        let asset_symbol = asset.weight.asset.name.clone();
                        let asset_total_quantity = order_liquidity
                            .asset_total_order_quantity
                            .get(&asset_symbol)?; // missing entry means we had math-overflow in previous step

                        // Weighted sum of asset liquidity for all index orders in batch
                        let asset_liquidity = order_liquidity
                            .asset_total_weighted_liquidity
                            .get(&asset_symbol)?; // missing entry means we had math-overflow in previous step

                        // Contribution fraction of this index order to total quantity
                        let asset_contribution_fraction =
                            safe!(asset_order_quantity / *asset_total_quantity)?;

                        // Liquidity portion pre-allocated based on contribution fraction
                        // This is just an estimate to start with some number
                        let asset_liquidity_contribution =
                            safe!(asset_contribution_fraction * *asset_liquidity)?;

                        //
                        // Formula:
                        //      index order fraction = quantity of asset liquidity available
                        //                           / quantity of asset in basket for index order
                        //
                        let temp_order_fraction =
                            safe!(asset_liquidity_contribution / asset_order_quantity)?;

                        // Take min(temp_order_fraction for all assets)
                        contribution.order_fraction =
                            contribution.order_fraction.min(temp_order_fraction);

                        //
                        // Formula:
                        //      possible index order quantity = index order fraction
                        //                                    * remaining index order quantity
                        //
                        contribution.order_quantity =
                            safe!(contribution.order_fraction * order_quantity)?;


                        println!(" * find_order_contribution: {} {:5} q={:0.5} tq={:0.5} tl={:0.5} acf={:0.5} alc={:0.5} of={:0.5} oq={:0.5}",
                            update.client_order_id,
                            asset_symbol, asset_order_quantity, asset_total_quantity, asset_liquidity,
                            asset_contribution_fraction,
                            asset_liquidity_contribution,
                            contribution.order_fraction,
                            contribution.order_quantity);

                        contribution
                            .asset_contribution_fraction
                            .insert(asset_symbol.clone(), asset_contribution_fraction).is_none().then_some(())?;
                        contribution
                            .asset_liquidity_contribution
                            .insert(asset_symbol, asset_liquidity_contribution).is_none().then_some(())
                    })
                    .count();

                if count != basket.basket_assets.len() {
                    // There was some error
                    None
                } else {
                    all_contributions.insert(update.client_order_id.clone(), contribution)
                    .is_none()
                    .then_some(())
                }
            });
            if let None = result {
                index_order.with_upgraded(|order| self.set_order_status(order, SolverOrderStatus::MathOverflow));
                false
            } else {
                true
            }
        });
        all_contributions
    }

    fn engage_orders(
        &self,
        more_orders: &mut MoreOrders,
        mut contributions: HashMap<ClientOrderId, FindOrderContribution>,
    ) -> Vec<RwLock<EngageOrder>> {
        println!("\nEngage Orders...");
        let mut enagaged_orders = Vec::new();
        more_orders
            .locked_orders
            .retain_mut(|(index_order_arc, index_order)| {
                let basket = more_orders
                    .baskets
                    .get(&index_order.symbol)
                    .expect("We should have basket at this stage");
                let index_price = more_orders
                    .index_prices
                    .get(&index_order.symbol)
                    .expect("We should have index prices at this stage");
                let result = Some(&index_order).and_then(|update| {
                    let contribution = contributions.remove(&update.client_order_id)?;
                    Some((update.client_order_id.clone(), contribution))
                });
                match result {
                    Some((client_order_id, contribution)) => {
                        let engaged_quantity = contribution.order_quantity;
                        enagaged_orders.push(RwLock::new(EngageOrder {
                            index_order: index_order_arc.clone(),
                            contribution,
                            address: index_order.address,
                            client_order_id,
                            symbol: index_order.symbol.clone(),
                            basket: basket.clone(),
                            engaged_side: index_order.side,
                            engaged_quantity,
                            engaged_price: *index_price,
                            filled_quantity: Amount::ZERO,
                        }));
                        index_order.with_upgraded(|order| {
                            self.set_order_status(order, SolverOrderStatus::Engaged)
                        });
                        true
                    }
                    _ => {
                        index_order.with_upgraded(|order| {
                            self.set_order_status(order, SolverOrderStatus::MathOverflow)
                        });
                        false
                    }
                }
            });

        enagaged_orders
    }
}

impl SolverStrategy for SimpleSolver {
    fn solve_engagements(
        &mut self,
        strategy_host: &dyn SolverStrategyHost,
    ) -> Option<crate::solver::solver::EngagedOrders> {
        let new_orders = strategy_host.get_order_batch();

        if new_orders.is_empty() {
            return None;
        }

        let mut more_orders = self.more_orders(&new_orders, strategy_host);

        // Get totals for assets in all Index Orders
        let order_liquidity = self.find_order_liquidity(&mut more_orders, strategy_host);

        // Now calculate how much liquidity is available per asset proportionally to order contribution
        let contributions = self.find_order_contribution(&mut more_orders, order_liquidity);

        // Now let's engage orders, and unlock them afterwards
        let engaged_orders = self.engage_orders(&mut more_orders, contributions);

        Some(EngagedOrders {
            batch_order_id: strategy_host.get_next_batch_order_id(),
            engaged_orders,
            baskets: more_orders.baskets,
            index_prices: more_orders.index_prices,
        })
    }
}
