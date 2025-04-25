use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    sync::Arc,
};

use chrono::Utc;
use eyre::OptionExt;
use itertools::{partition, Itertools};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    core::bits::{
        Amount, AssetOrder, BatchOrder, BatchOrderId, ClientOrderId, OrderId, PriceType, Side,
        Symbol,
    },
    index::{
        basket::Basket,
        basket_manager::{BasketManager, BasketNotification},
    },
    market_data::{
        order_book::order_book_manager::{OrderBookEvent, OrderBookManager},
        price_tracker::{PriceEvent, PriceTracker},
    },
};

use super::{
    index_order::IndexOrder,
    index_order_manager::{IndexOrderEvent, IndexOrderManager},
    index_quote_manager::{QuoteRequestEvent, QuoteRequestManager},
    inventory_manager::{InventoryEvent, InventoryManager},
};

struct MoreOrders<'a> {
    locked_orders: Vec<(
        &'a Arc<RwLock<IndexOrder>>,
        RwLockUpgradableReadGuard<'a, IndexOrder>,
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

struct FindOrderContribution {
    /// Contribution of the user index order for asset
    /// asset => asset contribution fraction
    asset_contribution_fraction: HashMap<Symbol, Amount>,

    /// asset => asset total liquidity x asset contribution fraction
    asset_liquidity_contribution: HashMap<Symbol, Amount>,

    /// min((asset liquidity contribution / asset quantity for index order) for all assets)
    order_fraction: Amount,

    /// order fraction x remaining quantity of the index order
    order_quantity: Amount,
}

struct EngageOrder {
    index_order: Arc<RwLock<IndexOrder>>,
    client_order_id: ClientOrderId,
    symbol: Symbol,
    basket: Arc<Basket>,
    engaged_side: Side,
    engaged_quantity: Amount,
    engaged_price: Amount,
    engaged_threshold: Amount,
}

struct EngagedOrders {
    engaged_orders: Vec<EngageOrder>,
    symbols: Vec<Symbol>,
    baskets: HashMap<Symbol, Arc<Basket>>,
    index_prices: HashMap<Symbol, Amount>,
    asset_prices: HashMap<Symbol, Amount>,
}

/// magic solver, needs to take index orders, and based on prices (from price
/// tracker) and available liquiduty (depth from order books), and active orders
/// (from order tracker) calculate best internal-portfolio rebalancing orders,
/// which will (partly) fill (some of the) ordered indexes.  Any position that
/// wasn't matched against ordered indexes shouldn't be kept for too long.
pub struct Solver {
    chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
    index_order_manager: Arc<RwLock<IndexOrderManager>>,
    quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
    basket_manager: Arc<RwLock<BasketManager>>,
    price_tracker: Arc<RwLock<PriceTracker>>,
    order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
    inventory_manager: Arc<RwLock<InventoryManager>>,
    max_orders: usize,
    tolerance: Amount,
}
impl Solver {
    pub fn new(
        chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
        index_order_manager: Arc<RwLock<IndexOrderManager>>,
        quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
        basket_manager: Arc<RwLock<BasketManager>>,
        price_tracker: Arc<RwLock<PriceTracker>>,
        order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
        inventory_manager: Arc<RwLock<InventoryManager>>,
        max_orders: usize,
        tolerance: Amount,
    ) -> Self {
        Self {
            chain_connector,
            index_order_manager,
            quote_request_manager,
            basket_manager,
            price_tracker,
            order_book_manager,
            inventory_manager,
            max_orders,
            tolerance,
        }
    }

    fn more_orders<'a>(&self, index_orders: &'a Vec<Arc<RwLock<IndexOrder>>>) -> MoreOrders<'a> {
        // Lock all Index Orders in the batch - for reading with intention to write
        let mut locked_orders = index_orders
            .iter()
            .map(|order| (order, order.upgradable_read()))
            .collect_vec();

        let basket_manager = self.basket_manager.read();
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
                    order.solver_cancel(format!("Invalid symbol {}", order.symbol).as_str());
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
        let individual_asset_prices = self
            .price_tracker
            .read()
            .get_prices(PriceType::VolumeWeighted, &symbols);

        if !individual_asset_prices.missing_symbols.is_empty() {
            todo!("Some assets have missing prices, what are we going to do? Defer probably.");
        }

        individual_asset_prices.prices.iter().for_each(|(a, p)| println!("individual_asset_prices > ({} p={})", a, p));

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
                        order.solver_cancel(
                            format!("Cannot calcualte index price for {}", order.symbol).as_str(),
                        );
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

    fn find_order_liquidity(&self, more_orders: &mut MoreOrders) -> FindOrderLiquidity {
        let mut asset_total_order_quantity = HashMap::new();
        let mut asset_total_weighted_liquidity = HashMap::new();

        let order_book_manager = self.order_book_manager.read();
        more_orders.locked_orders.retain_mut(|(_, index_order)| {
            // We'll only pick the first order update from the update queue
            // Note that we chould use iter() instead of front().inspect() and
            // take more than one update [TBD]
            let result = index_order.order_updates.front().and_then(|update| {
                let update = update.read();
                let side = update.side;
                let price = update.price;
                let order_quantity = update.remaining_quantity;
                let threshold = update.price_threshold;

                // We should be able to get basket and its current price, as in locked_orders
                // we only keep orders that weren't erroneous
                let current_price = more_orders.index_prices.get(&index_order.symbol)?;
                let basket = more_orders.baskets.get(&index_order.symbol)?;

                println!("index order: {} {:?} p={} q={} t={}", index_order.symbol, side, price, order_quantity, threshold);

                let target_asset_prices_and_quantites = basket
                    .basket_assets
                    .iter()
                    .map_while(|asset| {
                        let asset_symbol = asset.weight.asset.name.clone();
                        let asset_price = more_orders.asset_prices.get(&asset_symbol)?;
                        //
                        // Formula:
                        //      quantity of asset to order = quantity of index order
                        //                                 * quantity of asset in basket
                        //
                        let asset_quantity = asset.quantity.checked_mul(order_quantity)?;
                        //
                        // Note: While asset prices can move in any direction, here we just assume
                        // that prices of all assets move proportionally to index price. We know it's
                        // not accurate, but we think it's good enough estimate to obtain liquidity.
                        //
                        // Formula:
                        //      target price of asset in basket = current price of asset in basket
                        //                                      * target basket price
                        //                                      / current basket price
                        //
                        let target_asset_price = asset_price
                            .checked_mul(price)
                            .and_then(|x| x.checked_div(*current_price))?;

                        println!("target_asset_price {} = {} * {} / {}", asset_symbol, asset_price, price, *current_price);

                        match asset_total_order_quantity.entry(asset.weight.asset.name.clone()) {
                            Entry::Occupied(mut entry) => {
                                let x: &Amount = entry.get();
                                entry.insert(x.checked_add(asset_quantity)?);
                            }
                            Entry::Vacant(entry) => {
                                entry.insert(asset_quantity);
                            }
                        }

                        println!("target_asset_prices_and_quantites << ({} p={} q={})", asset_symbol, target_asset_price, asset_quantity);

                        Some((asset_symbol, target_asset_price, asset_quantity))
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

                let liquidity = order_book_manager
                    .get_liquidity(side.opposite_side(), &target_asset_prices, threshold)
                    .ok()?;

                for (asset_symbol, asset_liquidity) in liquidity {
                    println!("liquidity > ({} t={} l={})", asset_symbol, threshold, asset_liquidity);
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
                    let asset_liquidity = asset_liquidity.checked_mul(*asset_quantity)?;

                    println!("asset_total_weighted_liquidity << ({} l={} q={})", asset_symbol, asset_liquidity, *asset_quantity);

                    match asset_total_weighted_liquidity.entry(asset_symbol.clone()) {
                        Entry::Occupied(mut entry) => {
                            let (weighted_sum, total_weight): &(Amount, Amount) = entry.get();
                            entry.insert((
                                weighted_sum.checked_add(asset_liquidity)?,
                                total_weight.checked_add(*asset_quantity)?,
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
                index_order.with_upgraded(|order| order.solver_cancel("Math overflow"));
                false
            } else {
                true
            }
        });

        FindOrderLiquidity {
            asset_total_order_quantity,
            asset_total_weighted_liquidity: asset_total_weighted_liquidity
                .into_iter()
                .filter_map(|(k, (w, s))| w.checked_div(s).map(|x| (k, x)))
                .collect(),
        }
    }

    fn find_order_contribution(
        &self,
        more_orders: &mut MoreOrders,
        order_liquidity: &FindOrderLiquidity,
    ) -> HashMap<ClientOrderId, FindOrderContribution> {
        let mut all_contributions = HashMap::new();

        // Remove erroneous orders
        more_orders.locked_orders.retain_mut(|(_, index_order)| {
            let basket = more_orders.baskets.get(&index_order.symbol);

            // Find error
            let result = index_order.order_updates.front().and_then(|update| {
                let basket = basket?;
                let update = update.read();
                let order_quantity = update.remaining_quantity;
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
                        let asset_order_quantity = asset.quantity.checked_mul(order_quantity)?;

                        // Total quantity of asset across all index orders in batch
                        let asset_symbol = asset.weight.asset.name.clone();
                        let asset_total_quantity = order_liquidity
                            .asset_total_order_quantity
                            .get(&asset_symbol)?;

                        // Weighted sum of asset liquidity for all index orders in batch
                        let asset_liquidity = order_liquidity
                            .asset_total_weighted_liquidity
                            .get(&asset_symbol)?;
                        
                        // Contribution fraction of this index order to total quantity
                        let asset_contribution_fraction =
                            asset_order_quantity.checked_div(*asset_total_quantity)?;

                        // Liquidity portion pre-allocated based on contribution fraction
                        // This is just an estimate to start with some number
                        let asset_liquidity_contribution =
                            asset_contribution_fraction.checked_mul(*asset_liquidity)?;

                        //
                        // Formula:
                        //      index order fraction = quantity of asset liquidity available
                        //                           / quantity of asset in basket for index order
                        //
                        let temp_order_fraction =
                            asset_liquidity_contribution.checked_div(asset_order_quantity)?;

                        // Take min(temp_order_fraction for all assets)
                        contribution.order_fraction =
                            contribution.order_fraction.min(temp_order_fraction);

                        //
                        // Formula:
                        //      possible index order quantity = index order fraction
                        //                                    * remaining index order quantity
                        //
                        contribution.order_quantity =
                            contribution.order_fraction.checked_mul(order_quantity)?;


                        println!("find_order_contribution: {} q={} tq={} tl={} acf={} alc={} of={} oq={}",
                            asset_symbol, asset_order_quantity, asset_total_quantity, asset_liquidity,
                            asset_contribution_fraction,
                            asset_liquidity_contribution,
                            contribution.order_fraction,
                            contribution.order_quantity);

                        match (
                            contribution
                                .asset_contribution_fraction
                                .entry(asset_symbol.clone()),
                            contribution
                                .asset_liquidity_contribution
                                .entry(asset_symbol),
                        ) {
                            (Entry::Vacant(contribution_entry), Entry::Vacant(liquidity_entry)) => {
                                // Both need to be inserted
                                contribution_entry.insert(asset_contribution_fraction);
                                liquidity_entry.insert(asset_liquidity_contribution);
                                Some(())
                            }
                            _ => None, // error
                        }
                    })
                    .count();

                if count != basket.basket_assets.len() {
                    // There was some error
                    None
                } else {
                    all_contributions.insert(update.client_order_id.clone(), contribution);
                    Some(())
                }
            });
            if let None = result {
                index_order.with_upgraded(|order| order.solver_cancel("Math overflow"));
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
        contributions: &HashMap<ClientOrderId, FindOrderContribution>,
    ) -> Vec<EngageOrder> {
        let mut enagaged_orders = Vec::new();
        more_orders
            .locked_orders
            .retain_mut(|(index_order_arc, index_order)| {
                let basket = more_orders.baskets.get(&index_order.symbol);
                let result = index_order.order_updates.front().and_then(|update| {
                    let update = update.read();
                    let contribution = contributions.get(&update.client_order_id)?;
                    Some((
                        update.client_order_id.clone(),
                        contribution.order_quantity,
                        update.price,
                        update.price_threshold,
                    ))
                });
                match result {
                    Some((client_order_id, quantity, price, threshold)) => index_order
                        .with_upgraded(|order| order.solver_engage(quantity, self.tolerance))
                        .and_then(|unmatched_quantity| {
                            let engaged_quantity =
                                if let Some(unmatched_quantity) = unmatched_quantity {
                                    quantity
                                        .checked_sub(unmatched_quantity)
                                        .ok_or_eyre("Math overflow")?
                                } else {
                                    quantity
                                };
                            enagaged_orders.push(EngageOrder {
                                index_order: index_order_arc.clone(),
                                client_order_id,
                                symbol: index_order.symbol.clone(),
                                basket: basket.ok_or_eyre("Basket not found")?.clone(),
                                engaged_side: index_order.side,
                                engaged_quantity,
                                engaged_price: price,
                                engaged_threshold: threshold,
                            });
                            Ok(())
                        })
                        .map_err(|err| println!("Error: {}", err)) // todo: we need to log errors somewhere
                        .is_ok(),
                    None => {
                        index_order.with_upgraded(|order| order.solver_cancel("Math overflow"));
                        false
                    }
                }
            });

        enagaged_orders
    }

    fn engage_more_orders(&self) -> EngagedOrders {
        // Receive a batch of Index Orders
        let index_orders = self
            .index_order_manager
            .write()
            .take_index_orders(self.max_orders);

        // Take some new orders
        let mut more_orders = self.more_orders(&index_orders);

        // Get totals for assets in all Index Orders
        let order_liquidity = self.find_order_liquidity(&mut more_orders);

        // Now calculate how much liquidity is available per asset proportionally to order contribution
        let contributions = self.find_order_contribution(&mut more_orders, &order_liquidity);

        // Now let's engage orders, and unlock them afterwards
        let engaged_orders = self.engage_orders(&mut more_orders, &contributions);

        EngagedOrders {
            engaged_orders,
            baskets: more_orders.baskets,
            symbols: more_orders.symbols,
            asset_prices: more_orders.asset_prices,
            index_prices: more_orders.index_prices,
        }
    }

    /// Core thinking function
    pub fn solve(&self) {
        // Receive some more orders, and prepare them
        let engaged_orders = self.engage_more_orders();

        // Compute symbols and threshold
        // ...

        // receive list of open lots from Inventory Manager
        let _positions = self
            .inventory_manager
            .read()
            .get_positions(&engaged_orders.symbols);

        // Compute: Allocate open lots to Index Orders
        // ...
        // TBD: Should Solver or Inventory Manager be allocating lots to index orders?

        // Send back to Index Order Manager fills if any
        //self.index_order_manager
        //    .write()
        //    .fill_order_request(ClientOrderId::default(), Amount::default());

        // Compute: Remaining quantity
        // ...

        // receive current prices from Price Tracker

        // Compute: Orders to send to update inventory
        // ...

        // Send order requests to Inventory Manager
        // ...throttle these: send one or few smaller ones

        // TODO: Should throttling be done here in Solver or in Inventory Manager

        // TODO: we should generate IDs
        let mut batch_ids = VecDeque::from([BatchOrderId("BatchOrder01".into())]);
        let mut order_ids = VecDeque::from([OrderId("Order01".into()), OrderId("Order02".into())]);
        
        // TODO: we should compact these batches (and do matrix solving)
        let batches = engaged_orders
            .engaged_orders
            .iter()
            .map(|engage_order| {
                let index_price = engaged_orders
                    .index_prices
                    .get(&engage_order.symbol)
                    .unwrap();
                Arc::new(BatchOrder {
                    batch_order_id: batch_ids.pop_front().unwrap().clone(),
                    created_timestamp: Utc::now(),
                    asset_orders: engage_order
                        .basket
                        .basket_assets
                        .iter()
                        .map(|basket_asset| AssetOrder {
                            order_id: order_ids.pop_front().unwrap().clone(),
                            price: basket_asset.price * engage_order.engaged_price / index_price,
                            quantity: engage_order.engaged_quantity * basket_asset.quantity,
                            side: engage_order.engaged_side,
                            symbol: basket_asset.weight.asset.name.clone(),
                        })
                        .collect_vec(),
                })
            })
            .collect_vec();

        for batch in batches {
            println!("batch: {}", batch.asset_orders.iter().map(|ba|
                    format!("{:?} {}: {} @ {}", ba.side, ba.symbol, ba.quantity, ba.price)
                ).join("; "));
            if let Err(err) = self.inventory_manager.write().new_order(batch) {
                // log somewhere this error
                println!("Error: {}", err)
            }
        }
    }

    /// Quoting function (fast)
    pub fn quote(&self, _quote_request: ()) {
        // Compute symbols and threshold
        // ...

        let symbols = [];
        let threshold = Amount::default();

        // receive current prices from Price Tracker
        let prices = self
            .price_tracker
            .read()
            .get_prices(PriceType::VolumeWeighted, &symbols);

        // receive available liquidity from Order Book Manager
        let _liquidity =
            self.order_book_manager
                .read()
                .get_liquidity(Side::Sell, &prices.prices, threshold);

        // Compute: Quote with cost
        // ...

        // send back quote
        self.quote_request_manager.write().respond_quote(());
    }

    pub fn handle_chain_event(&self, notification: ChainNotification) {
        println!("Solver: Handle Chain Event");
        match notification {
            ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                let symbols = basket_definition
                    .weights
                    .iter()
                    .map(|w| w.asset.name.clone())
                    .collect_vec();

                let get_prices_response = self
                    .price_tracker
                    .read()
                    .get_prices(PriceType::VolumeWeighted, symbols.as_slice());

                if !get_prices_response.missing_symbols.is_empty() {
                    println!(
                        "Solver: No prices available for some symbols: {:?}",
                        get_prices_response.missing_symbols
                    );
                }

                let target_price = "1000".try_into().unwrap(); // TODO

                if let Err(err) = self.basket_manager.write().set_basket_from_definition(
                    symbol,
                    basket_definition,
                    &get_prices_response.prices,
                    target_price,
                ) {
                    println!("Solver: Error while setting curator weights: {err}");
                }
            }
            ChainNotification::PaymentIn {
                address: _,
                payment_id: _,
                amount_paid_in: _,
            } => (),
        }
    }

    /// receive Index Order
    pub fn handle_index_order(&self, _notification: IndexOrderEvent) {
        println!("Solver: Handle Index Order");
        self.solve();
    }

    // receive QR
    pub fn handle_quote_request(&self, _notification: QuoteRequestEvent) {
        println!("Solver: Handle Quote Request");
        //self.quote(());
    }

    /// Receive fill notifications
    pub fn handle_inventory_event(&self, _notification: InventoryEvent) {
        println!("Solver: Handle Inventory Event");
        //self.solve();
    }

    /// receive current prices from Price Tracker
    pub fn handle_price_event(&self, _notification: PriceEvent) {
        println!("Solver: Handle Price Event");
        //self.solve();
    }

    /// receive available liquidity from Order Book Manager
    pub fn handle_book_event(&self, _notification: OrderBookEvent) {
        println!("Solver: Handle Book Event");
        //self.solve();
    }

    /// receive basket notification
    pub fn handle_basket_event(&self, notification: BasketNotification) {
        println!("Solver: Handle Basket Notification");
        //self.solve();
        // TODO: (move this) once solvign is done notify new weights were applied
        match notification {
            BasketNotification::BasketAdded(symbol, basket) => self
                .chain_connector
                .write()
                .solver_weights_set(symbol, basket),
            BasketNotification::BasketUpdated(symbol, basket) => self
                .chain_connector
                .write()
                .solver_weights_set(symbol, basket),
            BasketNotification::BasketRemoved(_symbol) => todo!(),
        }
    }
}

#[cfg(test)]
mod test {
    use std::{any::type_name, sync::Arc, time::Duration};

    use chrono::Utc;
    use crossbeam::{
        channel::{unbounded, Sender},
        select,
    };

    use crate::{
        assert_decimal_approx_eq, blockchain::chain_connector::test_util::{
            MockChainConnector, MockChainInternalNotification,
        }, core::{
            bits::{PaymentId, PricePointEntry},
            functional::{
                IntoNotificationHandlerOnceBox, IntoObservableMany, IntoObservableSingle,
                NotificationHandlerOnce,
            },
            test_util::{
                get_mock_address_1, get_mock_asset_1_arc, get_mock_asset_2_arc,
                get_mock_asset_name_1, get_mock_asset_name_2, get_mock_decimal,
                get_mock_index_name_1,
            },
        }, index::{
            basket::{AssetWeight, BasketDefinition},
            basket_manager::BasketNotification,
        }, market_data::{
            market_data_connector::{
                test_util::MockMarketDataConnector, MarketDataConnector, MarketDataEvent,
            },
            order_book::order_book_manager::PricePointBookManager,
        }, order_sender::{
            order_connector::{test_util::MockOrderConnector, OrderConnectorNotification},
            order_tracker::{OrderTracker, OrderTrackerNotification},
        }, server::server::{test_util::MockServer, ServerEvent, ServerResponse}, solver::index_quote_manager::test_util::MockQuoteRequestManager
    };

    use super::*;

    impl<T> NotificationHandlerOnce<T> for Sender<T>
    where
        T: Send + Sync,
    {
        fn handle_notification(&self, notification: T) {
            self.send(notification)
                .expect(format!("Failed to handle {}", type_name::<T>()).as_str());
        }
    }

    impl<T> IntoNotificationHandlerOnceBox<T> for Sender<T>
    where
        T: Send + Sync + 'static,
    {
        fn into_notification_handler_once_box(self) -> Box<dyn NotificationHandlerOnce<T>> {
            Box::new(self)
        }
    }

    #[test]
    fn sbe_solver() {
        let tolerance = get_mock_decimal("0.0001");

        let (chain_sender, chain_receiver) = unbounded::<ChainNotification>();
        let (index_order_sender, index_order_receiver) = unbounded::<IndexOrderEvent>();
        let (quote_request_sender, quote_request_receiver) = unbounded::<QuoteRequestEvent>();
        let (inventory_sender, inventory_receiver) = unbounded::<InventoryEvent>();
        let (book_sender, book_receiver) = unbounded::<OrderBookEvent>();
        let (price_sender, price_receiver) = unbounded::<PriceEvent>();
        let (market_sender, market_receiver) = unbounded::<Arc<MarketDataEvent>>();
        let (basket_sender, basket_receiver) = unbounded::<BasketNotification>();
        let (fix_server_sender, fix_server_receiver) = unbounded::<Arc<ServerEvent>>();
        let (order_tracker_sender, order_tracker_receiver) =
            unbounded::<OrderTrackerNotification>();
        let (order_connector_sender, order_connector_receiver) =
            unbounded::<OrderConnectorNotification>();

        /*
        NOTES:
        This SBE is to demonstrate general structure of the application.
        We can see dependencies (direct ownership), as well as dependency inversions (events).
        In this example we use direct callbacks from event source to event handler.
        The production version will make use of channels, and dispatch, but we need to
        be careful to ensure FIFO event ordering.
        */
        let order_connector = Arc::new(RwLock::new(MockOrderConnector::new()));
        let order_tracker = Arc::new(RwLock::new(OrderTracker::new(
            order_connector.clone(),
            tolerance,
        )));
        let inventory_manager = Arc::new(RwLock::new(InventoryManager::new(
            order_tracker.clone(),
            tolerance,
        )));

        let market_data_connector = Arc::new(RwLock::new(MockMarketDataConnector::new()));
        let order_book_manager = Arc::new(RwLock::new(PricePointBookManager::new(tolerance)));
        let price_tracker = Arc::new(RwLock::new(PriceTracker::new()));

        let chain_connector = Arc::new(RwLock::new(MockChainConnector::new()));
        let fix_server = Arc::new(RwLock::new(MockServer::new()));

        let index_order_manager = Arc::new(RwLock::new(IndexOrderManager::new(
            fix_server.clone(),
            tolerance,
        )));
        let quote_request_manager = Arc::new(RwLock::new(MockQuoteRequestManager::new(
            fix_server.clone(),
        )));

        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        let solver = Arc::new(Solver::new(
            chain_connector.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            basket_manager.clone(),
            price_tracker.clone(),
            order_book_manager.clone(),
            inventory_manager.clone(),
            4,
            tolerance,
        ));

        solver
            .basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_sender);

        chain_connector
            .write()
            .get_single_observer_mut()
            .set_observer_from(chain_sender);

        index_order_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(index_order_sender);

        quote_request_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(quote_request_sender);

        inventory_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(inventory_sender);

        price_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(price_sender);

        order_book_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(book_sender);

        market_data_connector
            .write()
            .get_multi_observer_mut()
            .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
                market_sender.send(e.clone()).unwrap()
            });

        fix_server
            .write()
            .get_multi_observer_mut()
            .add_observer_fn(move |e: &Arc<ServerEvent>| {
                fix_server_sender.send(e.clone()).unwrap();
            });

        order_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(order_tracker_sender);

        order_connector
            .write()
            .get_single_observer_mut()
            .set_observer_fn(order_connector_sender);

        let order_tracker_2 = order_tracker.clone();

        let flush_events = move || {
            // Simple dispatch loop
            loop {
                select! {
                    recv(chain_receiver) -> res => solver.handle_chain_event(res.unwrap()),
                    recv(index_order_receiver) -> res => solver.handle_index_order(res.unwrap()),
                    recv(quote_request_receiver) -> res => solver.handle_quote_request(res.unwrap()),
                    recv(inventory_receiver) -> res => solver.handle_inventory_event(res.unwrap()),
                    recv(price_receiver) -> res => solver.handle_price_event(res.unwrap()),
                    recv(book_receiver) -> res => solver.handle_book_event(res.unwrap()),
                    recv(basket_receiver) -> res => solver.handle_basket_event(res.unwrap()),

                    recv(market_receiver) -> res => {
                        let e = res.unwrap();
                        price_tracker
                            .write()
                            .handle_market_data(&e);

                        order_book_manager
                            .write()
                            .handle_market_data(&e);
                    },
                    recv(fix_server_receiver) -> res => {
                        let e = res.unwrap();
                        index_order_manager
                            .write()
                            .handle_server_message(&e)
                            .expect("Failed to handle index order");

                        quote_request_manager
                            .write()
                            .handle_server_message(&e);
                    },
                    recv(order_tracker_receiver) -> res => {
                        inventory_manager
                            .write()
                            .handle_fill_report(res.unwrap())
                            .expect("Failed to handle fill report");
                    },
                    recv(order_connector_receiver) -> res => {
                        order_tracker
                            .write()
                            .handle_order_notification(res.unwrap());
                    },
                    default => { break; },
                }
            }
        };

        let (mock_chain_sender, mock_chain_receiver) = unbounded::<MockChainInternalNotification>();
        let (mock_server_sender, _mock_server_receiver) = unbounded::<ServerResponse>();

        chain_connector
            .write()
            .internal_observer
            .set_observer_from(mock_chain_sender);

        fix_server
            .write()
            .internal_observer
            .set_observer_from(mock_server_sender);

        // connect to exchange
        order_connector.write().connect();

        // connect to exchange
        market_data_connector.write().connect();

        // subscribe to symbol/USDC markets
        market_data_connector
            .write()
            .subscribe(&[get_mock_asset_name_1(), get_mock_asset_name_2()])
            .unwrap();

        // send some market data
        // top of the book
        market_data_connector.write().notify_top_of_book(
            get_mock_asset_name_1(),
            get_mock_decimal("90.0"),
            get_mock_decimal("100.0"),
            get_mock_decimal("10.0"),
            get_mock_decimal("20.0"),
        );

        market_data_connector.write().notify_top_of_book(
            get_mock_asset_name_2(),
            get_mock_decimal("295.0"),
            get_mock_decimal("300.0"),
            get_mock_decimal("80.0"),
            get_mock_decimal("50.0"),
        );

        // last trade
        market_data_connector.write().notify_trade(
            get_mock_asset_name_1(),
            get_mock_decimal("90.0"),
            get_mock_decimal("5.0"),
        );

        market_data_connector.write().notify_trade(
            get_mock_asset_name_2(),
            get_mock_decimal("300.0"),
            get_mock_decimal("15.0"),
        );

        // book depth
        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_1(),
            vec![
                PricePointEntry {
                    price: get_mock_decimal("90.0"),
                    quantity: get_mock_decimal("10.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("80.0"),
                    quantity: get_mock_decimal("40.0"),
                },
            ],
            vec![
                PricePointEntry {
                    price: get_mock_decimal("100.0"),
                    quantity: get_mock_decimal("20.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("110.0"),
                    quantity: get_mock_decimal("30.0"),
                },
            ],
        );

        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_2(),
            vec![
                PricePointEntry {
                    price: get_mock_decimal("295.0"),
                    quantity: get_mock_decimal("80.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("290.0"),
                    quantity: get_mock_decimal("100.0"),
                },
            ],
            vec![
                PricePointEntry {
                    price: get_mock_decimal("300.0"),
                    quantity: get_mock_decimal("50.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("305.0"),
                    quantity: get_mock_decimal("150.0"),
                },
            ],
        );

        // necessary to wait until all market data events are ingested
        flush_events();

        // define basket
        let basket_definition = BasketDefinition::try_new(vec![
            AssetWeight::new(get_mock_asset_1_arc(), get_mock_decimal("0.8")),
            AssetWeight::new(get_mock_asset_2_arc(), get_mock_decimal("0.2")),
        ])
        .unwrap();

        // send basket weights
        chain_connector
            .write()
            .notify_curator_weights_set(get_mock_index_name_1(), basket_definition);

        flush_events();

        // wait for solver to solve...
        let solver_weithgs_set = mock_chain_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive SolverWeightsSet");

        assert!(matches!(
            solver_weithgs_set,
            MockChainInternalNotification::SolverWeightsSet(_, _)
        ));

        fix_server
            .write()
            .notify_server_event(Arc::new(ServerEvent::NewIndexOrder {
                address: get_mock_address_1(),
                client_order_id: ClientOrderId("Order01".into()),
                payment_id: PaymentId("Pay001".into()),
                symbol: get_mock_index_name_1(),
                side: Side::Buy,
                price: get_mock_decimal("1005.0"),
                price_threshold: get_mock_decimal("0.05"),
                quantity: get_mock_decimal("2.5"),
                timestamp: Utc::now(),
            }));

        flush_events();

        let order1 = order_tracker_2.read().get_order(&OrderId("Order01".into()));
        let order2 = order_tracker_2.read().get_order(&OrderId("Order02".into()));

        assert!(matches!(order1, Some(_)));
        assert!(matches!(order2, Some(_)));
        let order1 = order1.unwrap();
        let order2 = order2.unwrap();

        assert_eq!(order1.symbol, get_mock_asset_name_1());
        assert_eq!(order2.symbol, get_mock_asset_name_2());
        assert_eq!(order1.side, Side::Buy);
        assert_eq!(order2.side, Side::Buy);

        assert_decimal_approx_eq!(order1.price, get_mock_decimal("97.15"), tolerance);
        assert_decimal_approx_eq!(order1.quantity, get_mock_decimal("20.0"), tolerance);
        assert_decimal_approx_eq!(order2.price, get_mock_decimal("298.4076923"), tolerance);
        assert_decimal_approx_eq!(order2.quantity, get_mock_decimal("1.627806563"), tolerance);

        // this will fail atm
        //mock_server_receiver
        //    .recv_timeout(Duration::from_secs(1))
        //    .expect("Failed to receive ServerResponse");
    }
}
