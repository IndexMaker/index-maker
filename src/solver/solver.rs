use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    sync::Arc,
};

use alloy::signers::k256::elliptic_curve::generic_array::functional;
use chrono::{DateTime, TimeDelta, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::{partition, Itertools};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use safe_math::safe;

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    core::{
        bits::{
            Address, Amount, AssetOrder, BatchOrder, BatchOrderId, ClientOrderId, OrderId,
            PaymentId, PriceType, Side, Symbol,
        },
        decimal_ext::DecimalExt,
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
    index_order_manager::{IndexOrderEvent, IndexOrderManager},
    index_quote_manager::{QuoteRequestEvent, QuoteRequestManager},
    inventory_manager::{InventoryEvent, InventoryManager},
    position::LotId,
};

enum IndexOrderStatus {
    Open,
    Engaged,
    Closed,
    InvalidSymbol,
    MathOverflow,
}

/// Solver's view of the Index Order
struct IndexOrderSolver {
    // ID of the original NewOrder request
    original_client_order_id: ClientOrderId,

    /// On-chain address of the User
    address: Address,

    // ID of the NewOrder request
    client_order_id: ClientOrderId,

    /// An ID of the on-chain payment
    payment_id: PaymentId,

    /// Symbol of an Index
    symbol: Symbol,

    /// Side of an order
    side: Side,

    /// Limit price
    price: Amount,

    /// Price max deviation %-age (as fraction) threshold
    price_threshold: Amount,

    /// Quantity remaining on the order to complete
    remaining_quantity: Amount,

    /// Quantity solver has engaged so far
    engaged_quantity: Amount,

    /// In-flight quantity in the sent order batch
    inflight_quantity: Amount,

    /// Quantity filled by subsequent batch order fills
    filled_quantity: Amount,

    /// Time when requested
    timestamp: DateTime<Utc>,

    // Solver status
    status: IndexOrderStatus,
}

struct BatchAssetTransaction {
    /// Asset Order ID
    order_id: OrderId,

    /// Transaction ID
    lot_id: LotId,

    /// Quantity filled
    quantity: Amount,

    /// Executed price
    price: Amount,

    /// Execution fee
    fee: Amount,

    /// Timestamp of execution
    timestamp: DateTime<Utc>,
}

struct BatchAssetPosition {
    /// Symbol of an asset
    pub symbol: Symbol,

    /// Side of an order
    pub side: Side,

    /// Position in this batch of this asset on that side
    /// Note: A batch can have both Buy and Sell orders,
    /// and we need separate position for them, as these
    /// will be matched for different users.
    pub position: Amount,

    /// Total quantity on all orders for this asset on that side
    pub order_quantity: Amount,

    /// Total price we paid for reaching the position
    pub realized_value: Amount,

    /// Total price we intended to pay on the order
    pub volley_size: Amount,

    /// Total fee we paid on the order
    pub fee: Amount,

    /// Time of the last transaction of this asset on this side
    pub last_update_timestamp: DateTime<Utc>,

    /// Transactions so far
    pub transactions: Vec<BatchAssetTransaction>,
}

struct BatchOrderStatus {
    // ID of the batch order
    pub batch_order_id: BatchOrderId,

    /// Positions of individual assets in this batch
    /// Note: These aren't our absolute positions, these are only positions
    /// of assets acquired/disposed in this batch
    positions: HashMap<(Symbol, Side), BatchAssetPosition>,

    /// Volley size (value of all orders in the batch)
    ///
    /// Note: We choose term "volley" and not "value" here:
    ///
    /// - Value: Carries the connotation of something you aim to preserve, hold
    ///     onto, or realize in a lasting way.
    ///
    /// - Volley: Implies a temporary burst, a collection meant to be processed
    ///     and then resolved or dispersed. It suggests an intent to move through a
    ///     state and then be "gotten rid of" in its initial form (either by
    ///     execution, cancellation, or the batch expiring)
    ///
    /// Then we will have total_volley_size across all batches, and for rate-limit
    /// we will have max_volley_size.
    ///
    volley_size: Amount,

    /// Filled volley (value of all fills across all orders in the batch)
    filled_volley: Amount,

    /// Fill-rate of this batch as a whole
    filled_fraction: Amount,

    /// Total price we paid for reaching the positions
    realized_value: Amount,

    /// Total fee we paid on the batch
    fee: Amount,

    /// Time of the last transaction
    last_update_timestamp: DateTime<Utc>,
}

struct CollateralTransaction {
    payment_id: PaymentId,
    amount: Amount,
    timestamp: DateTime<Utc>,
}

struct CollateralPosition {
    /// On-chain address of the User
    address: Address,

    /// Total balance of credits less debits in force
    balance: Amount,

    /// Total amount of pending credits
    pending_cr: Amount,

    /// Total amount of pending debits
    pending_dr: Amount,

    /// Pending credits
    transactions_cr: VecDeque<CollateralTransaction>,

    /// Pending debits
    transactions_dr: VecDeque<CollateralTransaction>,

    /// Completed credits and debits
    completed_transactions: VecDeque<CollateralTransaction>,

    /// Last time we updated this position
    last_update_timestamp: DateTime<Utc>,
}

impl CollateralPosition {
    fn new(address: Address) -> Self {
        Self {
            address,
            balance: Amount::ZERO,
            pending_cr: Amount::ZERO,
            pending_dr: Amount::ZERO,
            transactions_cr: VecDeque::new(),
            transactions_dr: VecDeque::new(),
            completed_transactions: VecDeque::new(),
            last_update_timestamp: Default::default(),
        }
    }
}

struct MoreOrders<'a> {
    locked_orders: Vec<(
        &'a Arc<RwLock<IndexOrderSolver>>,
        RwLockUpgradableReadGuard<'a, IndexOrderSolver>,
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
    index_order: Arc<RwLock<IndexOrderSolver>>,
    contribution: FindOrderContribution,
    address: Address,
    client_order_id: ClientOrderId,
    symbol: Symbol,
    basket: Arc<Basket>,
    engaged_side: Side,
    engaged_quantity: Amount,
    engaged_price: Amount,
    engaged_threshold: Amount,
    filled_quantity: Amount,
}

struct EngagedOrders {
    batch_order_id: BatchOrderId,
    engaged_orders: Vec<RwLock<EngageOrder>>,
    symbols: Vec<Symbol>,
    baskets: HashMap<Symbol, Arc<Basket>>,
    index_prices: HashMap<Symbol, Amount>,
    asset_prices: HashMap<Symbol, Amount>,
}

pub trait OrderIdProvider {
    fn next_order_id(&mut self) -> OrderId;
    fn next_batch_order_id(&mut self) -> BatchOrderId;
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
    order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
    client_orders: RwLock<HashMap<(Address, ClientOrderId), Arc<RwLock<IndexOrderSolver>>>>,
    client_funds: RwLock<HashMap<Address, Arc<RwLock<CollateralPosition>>>>,
    pending_funds: RwLock<VecDeque<Arc<RwLock<CollateralPosition>>>>,
    ready_orders: RwLock<VecDeque<Arc<RwLock<IndexOrderSolver>>>>,
    engagements: RwLock<HashMap<BatchOrderId, EngagedOrders>>,
    ready_batches: RwLock<VecDeque<BatchOrderId>>,
    batches: RwLock<HashMap<BatchOrderId, BatchOrderStatus>>,
    max_orders: usize,
    tolerance: Amount,
    fund_wait_period: TimeDelta,
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
        order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
        max_orders: usize,
        tolerance: Amount,
        fund_wait_period: TimeDelta,
    ) -> Self {
        Self {
            chain_connector,
            index_order_manager,
            quote_request_manager,
            basket_manager,
            price_tracker,
            order_book_manager,
            inventory_manager,
            order_id_provider,
            client_orders: RwLock::new(HashMap::new()),
            client_funds: RwLock::new(HashMap::new()),
            pending_funds: RwLock::new(VecDeque::new()),
            ready_orders: RwLock::new(VecDeque::new()),
            engagements: RwLock::new(HashMap::new()),
            ready_batches: RwLock::new(VecDeque::new()),
            batches: RwLock::new(HashMap::new()),
            max_orders,
            tolerance,
            fund_wait_period,
        }
    }

    fn set_order_status(&self, order: &mut IndexOrderSolver, status: IndexOrderStatus) {
        order.status = status;
    }

    fn more_orders<'a>(
        &self,
        index_orders: &'a Vec<Arc<RwLock<IndexOrderSolver>>>,
    ) -> MoreOrders<'a> {
        println!("\nMore Orders...");
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
                    self.set_order_status(order, IndexOrderStatus::InvalidSymbol);
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
                        self.set_order_status(order, IndexOrderStatus::MathOverflow);
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
        println!("\nFind Order Liquidity...");
        println!("   q   quantity");
        println!("   p   price");
        println!("   t   threshold");
        println!("   l   liquidity");
        println!("");
        let mut asset_total_order_quantity = HashMap::new();
        let mut asset_total_weighted_liquidity = HashMap::new();

        let order_book_manager = self.order_book_manager.read();
        more_orders.locked_orders.retain_mut(|(_, index_order)| {
            let result = Some(&index_order).and_then(|update| {
                let side = update.side;
                let price = update.price;
                let order_quantity = update.remaining_quantity;
                let threshold = update.price_threshold;

                // We should be able to get basket and its current price, as in locked_orders
                // we only keep orders that weren't erroneous
                let current_price = more_orders.index_prices.get(&index_order.symbol)?;
                let basket = more_orders.baskets.get(&index_order.symbol)?;

                println!(
                    " * index order: {:5} {} < {} {:?} p={:0.5} q={:0.5} t={:0.5}",
                    index_order.symbol,
                    index_order.original_client_order_id,
                    update.client_order_id,
                    side,
                    price,
                    order_quantity,
                    threshold
                );

                println!("\n- Find target asset prices and quantites...\n");
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
                        let asset_quantity = safe!(asset.quantity * order_quantity)?;

                        println!(
                            " * asset_quantity {:5} {:0.5} = {:0.5} * {:0.5}",
                            asset_symbol, asset_quantity, asset.quantity, order_quantity
                        );

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
                        let target_asset_price =
                            safe!(safe!(*asset_price * price) / *current_price)?;

                        println!(
                            " * target_asset_price {:5} {:0.5} = {:0.5} * {:0.5} / {:0.5}",
                            asset_symbol, target_asset_price, asset_price, price, *current_price
                        );

                        match asset_total_order_quantity.entry(asset.weight.asset.name.clone()) {
                            Entry::Occupied(mut entry) => {
                                let x: &Amount = entry.get();
                                entry.insert(safe!(*x + asset_quantity)?);
                            }
                            Entry::Vacant(entry) => {
                                entry.insert(asset_quantity);
                            }
                        }

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
                    self.set_order_status(order, IndexOrderStatus::MathOverflow)
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
                        let asset_order_quantity = safe!(asset.quantity * order_quantity)?;

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
                index_order.with_upgraded(|order| self.set_order_status(order, IndexOrderStatus::MathOverflow));
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
                let basket = more_orders.baskets.get(&index_order.symbol);
                let result = Some(&index_order).and_then(|update| {
                    let contribution = contributions.remove(&update.client_order_id)?;
                    Some((
                        update.client_order_id.clone(),
                        contribution,
                        update.price,
                        update.price_threshold,
                    ))
                });
                match (result, basket) {
                    (Some((client_order_id, contribution, price, threshold)), Some(basket)) => {
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
                            engaged_price: price,
                            engaged_threshold: threshold,
                            filled_quantity: Amount::ZERO,
                        }));
                        true
                    }
                    _ => {
                        index_order.with_upgraded(|order| {
                            self.set_order_status(order, IndexOrderStatus::MathOverflow)
                        });
                        false
                    }
                }
            });

        enagaged_orders
    }

    fn do_engage_more_orders(&self) -> Option<EngagedOrders> {
        println!("Engage More Orders...");

        // Receive a batch of new Index Orders
        let new_orders = (|| {
            let mut new_orders = self.ready_orders.write();
            let max_drain = new_orders.len().min(self.max_orders);
            new_orders.drain(..max_drain).collect_vec()
        })();

        if new_orders.is_empty() {
            return None;
        }

        println!("There are some engaged orders");

        let mut more_orders = self.more_orders(&new_orders);

        // Get totals for assets in all Index Orders
        let order_liquidity = self.find_order_liquidity(&mut more_orders);

        // Now calculate how much liquidity is available per asset proportionally to order contribution
        let contributions = self.find_order_contribution(&mut more_orders, order_liquidity);

        // Now let's engage orders, and unlock them afterwards
        let engaged_orders = self.engage_orders(&mut more_orders, contributions);

        // TODO: Generate it!
        let batch_order_id = self.order_id_provider.write().next_batch_order_id();

        Some(EngagedOrders {
            batch_order_id,
            engaged_orders,
            baskets: more_orders.baskets,
            symbols: more_orders.symbols,
            asset_prices: more_orders.asset_prices,
            index_prices: more_orders.index_prices,
        })
    }

    fn engage_more_orders(&self) -> Result<()> {
        // Receive some more orders, and prepare them
        if let Some(engaged_orders) = self.do_engage_more_orders() {
            let send_engage = engaged_orders
                .engaged_orders
                .iter()
                .map(|order| {
                    let order = order.write();
                    (
                        order.address,
                        order.client_order_id.clone(),
                        order.symbol.clone(),
                        order.engaged_quantity,
                    )
                })
                .collect_vec();

            let batch_order_id = match self
                .engagements
                .write()
                .entry(engaged_orders.batch_order_id.clone())
            {
                Entry::Vacant(entry) => entry.insert(engaged_orders).batch_order_id.clone(),
                Entry::Occupied(_) => Err(eyre!("Dublicate Batch Id"))?,
            };

            self.index_order_manager
                .write()
                .engage_orders(batch_order_id, send_engage)?;
        }
        Ok(())
    }

    fn send_batch(&self, engaged_orders: &EngagedOrders, timestamp: DateTime<Utc>) -> Result<()> {
        // TODO: we should generate IDs
        let batch_order_id = &engaged_orders.batch_order_id;

        // TODO: we should compact these batches (and do matrix solving)
        let batches = engaged_orders
            .engaged_orders
            .iter()
            .map_while(|engage_order| {
                let engage_order = engage_order.read();
                let mut index_order_write = engage_order.index_order.write();
                index_order_write.inflight_quantity =
                    safe!(index_order_write.inflight_quantity + engage_order.engaged_quantity)?;
                let index_price = engaged_orders
                    .index_prices
                    .get(&engage_order.symbol)
                    .unwrap();
                Some(Arc::new(BatchOrder {
                    batch_order_id: batch_order_id.clone(),
                    created_timestamp: timestamp,
                    asset_orders: engage_order
                        .basket
                        .basket_assets
                        .iter()
                        .map(|basket_asset| AssetOrder {
                            order_id: self.order_id_provider.write().next_order_id(),
                            price: basket_asset.price * engage_order.engaged_price / index_price,
                            quantity: engage_order.engaged_quantity * basket_asset.quantity,
                            side: engage_order.engaged_side,
                            symbol: basket_asset.weight.asset.name.clone(),
                        })
                        .collect_vec(),
                }))
            })
            .collect_vec();

        for batch in batches {
            println!(
                "Sending Batch: {}",
                batch
                    .asset_orders
                    .iter()
                    .map(|ba| format!(
                        "{:?} {}: {:0.5} @ {:0.5}",
                        ba.side, ba.symbol, ba.quantity, ba.price
                    ))
                    .join("; ")
            );
            let mut batch_order_status = BatchOrderStatus {
                batch_order_id: batch.batch_order_id.clone(),
                positions: HashMap::new(),
                volley_size: Amount::ZERO,
                filled_volley: Amount::ZERO,
                filled_fraction: Amount::ZERO,
                realized_value: Amount::ZERO,
                fee: Amount::ZERO,
                last_update_timestamp: batch.created_timestamp,
            };

            for order in &batch.asset_orders {
                let key = (order.symbol.clone(), order.side);
                match batch_order_status.positions.entry(key) {
                    Entry::Occupied(mut entry) => {
                        let position = entry.get_mut();
                        position.order_quantity = safe!(position.order_quantity + order.quantity)
                            .ok_or_eyre("Math Problem")?;
                    }
                    Entry::Vacant(entry) => {
                        let position = BatchAssetPosition {
                            symbol: order.symbol.clone(),
                            side: order.side,
                            order_quantity: order.quantity,
                            volley_size: safe!(order.price * order.quantity)
                                .ok_or_eyre("Math Problem")?,
                            position: Amount::ZERO,
                            realized_value: Amount::ZERO,
                            fee: Amount::ZERO,
                            last_update_timestamp: batch.created_timestamp,
                            transactions: Vec::new(),
                        };
                        batch_order_status.volley_size =
                            safe!(batch_order_status.volley_size + position.volley_size)
                                .ok_or_eyre("Math Problem")?;
                        entry.insert(position);
                    }
                }
            }

            self.batches
                .write()
                .insert(batch.batch_order_id.clone(), batch_order_status)
                .is_none()
                .then_some(())
                .ok_or_eyre("Duplicate batch ID")?;

            self.inventory_manager.write().new_order(batch)?;
        }
        Ok(())
    }

    fn send_more_batches(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let engagements = self.engagements.read();

        let new_engagements = (|| {
            let mut new_batches = self.ready_batches.write();
            let max_drain = new_batches.len().min(self.max_orders);
            new_batches
                .drain(..max_drain)
                .filter_map(|batch_order_id| engagements.get(&batch_order_id))
                .collect_vec()
        })();

        for engaged_orders in new_engagements {
            self.send_batch(engaged_orders, timestamp)?;
        }

        Ok(())
    }

    fn fill_index_order(
        &self,
        batch: &mut BatchOrderStatus,
        engaged_order: &RwLock<EngageOrder>,
    ) -> Result<Option<Arc<RwLock<IndexOrderSolver>>>> {
        let mut engaged_order = engaged_order.upgradable_read();
        let engaged_quantity = engaged_order.engaged_quantity;
        let index_order = engaged_order.index_order.clone();
        let mut index_order_write = index_order.write();

        // We search for fill-rate of this Index Order matching against
        // available lots in this batch. Note that this is fill-rate in
        // this batch, and 100% means that only the fraction of the Index Order
        // that was included in this batch is fully filled, and there might
        // still be more quantity outside of this batch on this Index Order
        // that will need to be filled at later time in another batch.
        let mut fill_rate = None;

        for asset in &engaged_order.basket.basket_assets {
            // = Amount of an asset in Basket Definition
            //  * IndexOrder quantity engaged in this batch
            let asset_quantity =
                safe!(asset.quantity * engaged_quantity).ok_or_eyre("Math Problem")?;

            let asset_symbol = &asset.weight.asset.name;
            let key = (asset_symbol.clone(), index_order_write.side);
            let position = batch
                .positions
                .get(&key)
                .ok_or_eyre("Missing position for asset")?;

            let contribution_fraction = *engaged_order
                .contribution
                .asset_contribution_fraction
                .get(asset_symbol)
                .ok_or_eyre("Asset contribution fraction not found")?;

            // = Current available position from this Batch
            //  * Index Order contribution fraction in this batch
            let available_quantity =
                safe!(position.position * contribution_fraction).ok_or_eyre("Math Problem")?;

            // = Quantity available in this batch for this Index Order
            //  / Quantity required to fully fill the fraction of the whole Index Order requested in this batch
            let avialable_fill_rate =
                safe!(available_quantity / asset_quantity).ok_or_eyre("Math Problem")?;

            // We're finding lowest fill-rate for this Index Order across all assets, because
            // we cannot fill this Index Order more than least available asset fill-rate.
            fill_rate = fill_rate.map_or(Some(avialable_fill_rate), |x: Amount| {
                Some(x.min(avialable_fill_rate))
            });
            println!(
                "{:5} q={:0.5} p={:0.5} aq={:0.5} cf={:0.5} afr={:0.5}",
                asset_symbol,
                asset_quantity,
                position.position,
                available_quantity,
                contribution_fraction,
                avialable_fill_rate
            );
        }

        // This is new filled quantity of this batch engagement with Index Order
        let filled_quantity = safe!(fill_rate * engaged_quantity).ok_or_eyre("Math Problem")?;

        // This is how much it has changed since last time it was updated
        let filled_quantity_delta =
            safe!(filled_quantity - engaged_order.filled_quantity).ok_or_eyre("Math Problem")?;

        // Now we update it
        engaged_order.with_upgraded(|x| {
            x.filled_quantity = filled_quantity;
        });

        println!(
            "Fill Index Order: {} {:0.5} (+{:0.5} {:0.3}%)",
            index_order_write.original_client_order_id,
            filled_quantity,
            filled_quantity_delta,
            safe!(fill_rate * Amount::ONE_HUNDRED).ok_or_eyre("Math Problem")?
        );

        if self.tolerance < filled_quantity_delta {
            // And we add the delta to the Index Order filled quantity
            index_order_write.filled_quantity =
                safe!(index_order_write.filled_quantity + filled_quantity_delta)
                    .ok_or_eyre("Math Problem")?;

            index_order_write.engaged_quantity =
                safe!(index_order_write.engaged_quantity - filled_quantity_delta)
                    .ok_or_eyre("Math Problem")?;

            let remaining_quantity =
                safe!(index_order_write.remaining_quantity + index_order_write.engaged_quantity)
                    .ok_or_eyre("Math Problem")?;

            println!(
                "IndexOrder (Solver): ifq={:0.5} irq={:0.5} ieq={:0.5} rq={:0.5}",
                index_order_write.filled_quantity,
                index_order_write.remaining_quantity,
                index_order_write.engaged_quantity,
                remaining_quantity
            );

            self.index_order_manager.write().fill_order_request(
                &index_order_write.address,
                &index_order_write.original_client_order_id,
                &index_order_write.symbol,
                filled_quantity_delta,
                batch.last_update_timestamp,
            )?;

            if remaining_quantity < self.tolerance {
                self.chain_connector.write().mint_index(
                    index_order_write.symbol.clone(),
                    index_order_write.filled_quantity,
                    index_order_write.address,
                    Amount::ZERO,
                    batch.last_update_timestamp,
                );
            }
            else if safe!(Amount::ONE - self.tolerance).ok_or_eyre("Math Problem")?
                < fill_rate.expect("Fill-rate Must have been known at this stage")
            {
                println!("IndexOrder batch fraction fully filled");
                return Ok(Some(index_order.clone()));
            }
        };

        Ok(None)
    }

    fn fill_batch_orders(&self, batch: &mut BatchOrderStatus) -> Result<()> {
        let engagements_read = self.engagements.read();
        let engagement = engagements_read
            .get(&batch.batch_order_id)
            .ok_or_else(|| eyre!("Engagement not found {}", batch.batch_order_id))?;

        let mut ready_orders = VecDeque::new();

        for engaged_order in &engagement.engaged_orders {
            if let Some(index_order) = self.fill_index_order(batch, engaged_order)? {
                ready_orders.push_back(index_order);
            }
        }

        self.ready_orders.write().extend(ready_orders.drain(..));
        Ok(())
    }

    fn process_credits(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let ready_timestamp = timestamp - self.fund_wait_period;
        let check_not_ready = |x: &CollateralTransaction| ready_timestamp < x.timestamp;
        let ready_funds = (|| {
            let mut funds = self.pending_funds.write();
            let res = funds.iter().find_position(|p| {
                p.read()
                    .transactions_cr
                    .front()
                    .map(check_not_ready)
                    .unwrap_or(true)
            });
            if let Some((pos, _)) = res {
                funds.drain(..pos)
            } else {
                funds.drain(..)
            }
            .collect_vec()
        })();

        for fund in ready_funds {
            let mut fund_write = fund.write();
            let res = fund_write
                .transactions_cr
                .iter()
                .find_position(|x| check_not_ready(x));
            let completed = if let Some((pos, _)) = res {
                fund_write.transactions_cr.drain(..pos)
            } else {
                fund_write.transactions_cr.drain(..)
            }
            .collect_vec();
            let mut balance = fund_write.balance;
            for tx in &completed {
                balance = safe!(balance + tx.amount).ok_or_eyre("Math Problem")?;
            }
            fund_write.balance = balance;
            fund_write.last_update_timestamp = ready_timestamp;
            fund_write.completed_transactions.extend(completed);
            println!(
                "New balance for {} {:0.5}",
                fund_write.address, fund_write.balance
            );
        }

        Ok(())
    }

    /// Core thinking function
    pub fn solve(&self, timestamp: DateTime<Utc>) {
        println!("\nSolve...");

        //
        // check if there is some collateral we could use
        //
        if let Err(err) = self.process_credits(timestamp) {
            eprintln!("Error while processing credits: {}", err);
        }

        //
        // NOTE: We should only engage new orders, and currently not much engaged
        // otherwise currently engaged orders may consume the liquidity.
        //
        // TODO: We may also track open liquidity promised to open orders
        //

        if let Err(err) = self.engage_more_orders() {
            eprintln!("Error while engaging more orders: {}", err);
        }

        //
        // NOTE: Index Order Manager will fire EngageIndexOrder event(s) and
        // we should move orders to new queue then, and here we could draw from
        // that new queue.
        //
        // So essentially:
        //  ( NewIndexOrder event ) => ready_queue =( find liquidity & engage )=> pending_queue
        //  ( EngageIndexOrder event ) => pending_queue =( move )=> engaged_queue
        //  ( solve ) => engaged_queue =( send order batch )=>  live_order_map
        //  ( inventory event ) => live_order_map =( match fill )=>

        // Compute symbols and threshold
        // ...

        // receive list of open lots from Inventory Manager
        //let _positions = self
        //    .inventory_manager
        //    .read()
        //    .get_positions(&engaged_orders.symbols);

        println!("Compute...");
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

        println!("\nSend Order Batches...");
        if let Err(err) = self.send_more_batches(timestamp) {
            eprintln!("Error while sending more batches: {}", err);
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

    pub fn handle_chain_event(&self, notification: ChainNotification) -> Result<()> {
        match notification {
            ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                println!("Solver: Handle Chain Event CuratorWeigthsSet {}", symbol);
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
                Ok(())
            }
            ChainNotification::Deposit {
                address,
                payment_id,
                amount,
                timestamp,
            } => {
                let collateral_position = self
                    .client_funds
                    .write()
                    .entry(address)
                    .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
                    .clone();

                (|| -> Result<()> {
                    let mut p = collateral_position.write();

                    p.transactions_cr.push_back(CollateralTransaction {
                        payment_id,
                        amount,
                        timestamp,
                    });
                    p.pending_cr = safe!(p.pending_cr + amount).ok_or_eyre("Math Problem")?;
                    p.last_update_timestamp = timestamp;
                    Ok(())
                })()?;

                self.pending_funds.write().push_back(collateral_position);
                Ok(())
            }
            ChainNotification::WithdrawalRequest {
                address,
                payment_id,
                amount,
                timestamp,
            } => {
                let collateral_position = self
                    .client_funds
                    .write()
                    .entry(address)
                    .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
                    .clone();

                (|| -> Result<()> {
                    let mut p = collateral_position.write();

                    p.transactions_dr.push_back(CollateralTransaction {
                        payment_id,
                        amount,
                        timestamp,
                    });
                    p.pending_dr = safe!(p.pending_dr + amount).ok_or_eyre("Math Problem")?;
                    p.last_update_timestamp = timestamp;
                    Ok(())
                })()?;

                self.pending_funds.write().push_back(collateral_position);
                Ok(())
            }
        }
    }

    /// receive Index Order
    pub fn handle_index_order(&self, notification: IndexOrderEvent) {
        match notification {
            IndexOrderEvent::NewIndexOrder {
                original_client_order_id,
                address,
                client_order_id,
                payment_id,
                symbol,
                side,
                price,
                price_threshold,
                quantity,
                timestamp,
            } => {
                println!(
                    "\nSolver: Handle Index Order NewIndexOrder {} {} < {} from {}",
                    symbol, original_client_order_id, client_order_id, address
                );
                match self
                    .client_orders
                    .write()
                    .entry((address, original_client_order_id.clone()))
                {
                    Entry::Vacant(entry) => {
                        let solver_order = Arc::new(RwLock::new(IndexOrderSolver {
                            original_client_order_id: original_client_order_id.clone(),
                            address,
                            client_order_id,
                            payment_id,
                            symbol,
                            side,
                            price,
                            price_threshold,
                            remaining_quantity: quantity,
                            engaged_quantity: Amount::ZERO,
                            inflight_quantity: Amount::ZERO,
                            filled_quantity: Amount::ZERO,
                            timestamp,
                            status: IndexOrderStatus::Open,
                        }));
                        entry.insert(solver_order.clone());
                        self.ready_orders.write().push_back(solver_order);
                    }
                    Entry::Occupied(_) => {
                        todo!();
                    }
                }
            }
            IndexOrderEvent::UpdateIndexOrder {
                original_client_order_id,
                address,
                client_order_id,
                quantity_removed: _,
                quantity_remaining: _,
                timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Index Order UpdateIndexOrder{} < {} from {}",
                    original_client_order_id, client_order_id, address
                );
                todo!();
            }
            IndexOrderEvent::EngageIndexOrder {
                batch_order_id,
                engaged_orders,
                timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Index Order EngageIndexOrder {}",
                    batch_order_id
                );
                match self.engagements.write().get_mut(&batch_order_id) {
                    Some(engaged_orders_stored) => {
                        engaged_orders_stored
                            .engaged_orders
                            .retain_mut(|engaged_order_stored| {
                                let mut engaged_order_stored = engaged_order_stored.write();
                                let index_order = engaged_order_stored.index_order.clone();
                                let mut index_order_stored = index_order.write();
                                match engaged_orders.get(&(
                                    engaged_order_stored.address,
                                    engaged_order_stored.client_order_id.clone(),
                                )) {
                                    Some(engaged_order) => {
                                        index_order_stored.remaining_quantity =
                                            engaged_order.quantity_remaining;
                                        index_order_stored.engaged_quantity =
                                            engaged_order.quantity_engaged;
                                        engaged_order_stored.engaged_quantity =
                                            engaged_order.quantity_engaged;
                                    }
                                    None => {
                                        self.set_order_status(
                                            &mut index_order_stored,
                                            IndexOrderStatus::MathOverflow,
                                        );
                                    }
                                }
                                true
                            });
                    }
                    None => {
                        todo!()
                    }
                }
                self.ready_batches.write().push_back(batch_order_id);
            }
            IndexOrderEvent::CancelIndexOrder {
                original_client_order_id,
                address,
                client_order_id,
                timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Index Order CancelIndexOrder {} < {} from {}",
                    original_client_order_id, client_order_id, address
                );
                todo!();
            }
        }
    }

    // receive QR
    pub fn handle_quote_request(&self, _notification: QuoteRequestEvent) {
        println!("\nSolver: Handle Quote Request");
        //self.quote(());
    }

    /// Receive fill notifications
    pub fn handle_inventory_event(&self, notification: InventoryEvent) -> Result<()> {
        match notification {
            InventoryEvent::OpenLot {
                order_id,
                batch_order_id,
                lot_id,
                symbol,
                side,
                price,
                quantity,
                fee,
                original_batch_quantity: _,
                batch_quantity_remaining: _,
                timestamp,
            } => {
                println!(
                    "\nSolver: Handle Inventory Event OpenLot {:?} {:5} {:0.5} @ {:0.5} + fee {:0.5} ({:0.3}%)",
                    side,
                    symbol,
                    quantity,
                    price,
                    fee,
                    (|| safe!(safe!(fee * Amount::ONE_HUNDRED) / safe!(quantity * price)?))().ok_or_eyre("Math Problem")?
                );
                let mut write_batches = self.batches.write();
                let batch = write_batches
                    .get_mut(&batch_order_id)
                    .ok_or_eyre("Missing Batch")?;

                let key = (symbol.clone(), side);
                let position = batch
                    .positions
                    .get_mut(&key)
                    .ok_or_eyre("Missing Position")?;

                (|| {
                    position.position = safe!(position.position + quantity)?;

                    let fraction_delta = safe!(quantity / position.order_quantity)?;
                    let filled_asset_volley = safe!(fraction_delta * position.volley_size)?;
                    batch.filled_volley = safe!(batch.filled_volley + filled_asset_volley)?;
                    batch.filled_fraction = safe!(batch.filled_volley / batch.volley_size)?;

                    let filled_value = safe!(quantity * price)?;
                    position.realized_value = safe!(position.realized_value + filled_value)?;
                    batch.realized_value = safe!(batch.realized_value + filled_value)?;

                    position.fee = safe!(position.fee + fee)?;
                    batch.fee = safe!(batch.fee + fee)?;

                    position.transactions.push(BatchAssetTransaction {
                        order_id,
                        lot_id,
                        quantity,
                        price,
                        fee,
                        timestamp,
                    });

                    position.last_update_timestamp = timestamp;

                    println!(
                        "Batch Position: {:?} {:5} price={:0.5} volley={:0.5} real={:0.5} + fee={:0.5}",
                        position.side,
                        position.symbol,
                        position.order_quantity,
                        position.volley_size,
                        position.realized_value,
                        position.fee
                    );

                    println!(
                        "Batch Status: {} volley={:0.5} fill={:0.5} frac={:0.5} real={:0.5} fee={:0.5}",
                        batch_order_id,
                        batch.volley_size,
                        batch.filled_volley,
                        batch.filled_fraction,
                        batch.realized_value,
                        batch.fee);
                    Some(())
                })()
                .ok_or_eyre("Math Problem")?;

                batch.last_update_timestamp = timestamp;

                self.fill_batch_orders(batch)
            }
            InventoryEvent::CloseLot {
                original_order_id: _,
                original_batch_order_id: _,
                original_lot_id: _,
                closing_order_id: _,
                closing_batch_order_id: _,
                closing_lot_id: _,
                symbol,
                side,
                original_price: _,
                closing_price,
                closing_fee,
                quantity_closed,
                original_quantity,
                quantity_remaining,
                closing_batch_original_quantity: _,
                closing_batch_quantity_remaining: _,
                original_timestamp: _,
                closing_timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Inventory Event CloseLot {:?} {:5} {:0.5}@{:0.5}+{:0.5} ({:0.5}%)",
                    side,
                    symbol,
                    quantity_closed,
                    closing_price,
                    closing_fee,
                    Amount::ONE_HUNDRED * (original_quantity - quantity_remaining)
                        / original_quantity
                );
                Ok(())
            }
        }
    }

    /// receive current prices from Price Tracker
    pub fn handle_price_event(&self, notification: PriceEvent) {
        match notification {
            PriceEvent::PriceChange { symbol } => {
                println!("Solver: Handle Price Event {:5}", symbol)
            }
        };
    }

    /// receive available liquidity from Order Book Manager
    pub fn handle_book_event(&self, notification: OrderBookEvent) {
        match notification {
            OrderBookEvent::BookUpdate { symbol } => {
                println!("Solver: Handle Book Event {:5}", symbol);
            }
            OrderBookEvent::UpdateError { symbol, error } => {
                println!("Solver: Handle Book Event {:5}, Error: {}", symbol, error);
            }
        }
    }

    /// receive basket notification
    pub fn handle_basket_event(&self, notification: BasketNotification) {
        // TODO: (move this) once solvign is done notify new weights were applied
        match notification {
            BasketNotification::BasketAdded(symbol, basket) => {
                println!("Solver: Handle Basket Notification BasketAdded {}", symbol);
                self.chain_connector
                    .write()
                    .solver_weights_set(symbol, basket)
            }
            BasketNotification::BasketUpdated(symbol, basket) => {
                println!(
                    "Solver: Handle Basket Notification BasketUpdated {}",
                    symbol
                );
                self.chain_connector
                    .write()
                    .solver_weights_set(symbol, basket)
            }
            BasketNotification::BasketRemoved(symbol) => {
                println!(
                    "Solver: Handle Basket Notification BasketRemoved {}",
                    symbol
                );
                todo!()
            }
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
    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        blockchain::chain_connector::test_util::{
            MockChainConnector, MockChainInternalNotification,
        },
        core::{
            bits::{PricePointEntry, SingleOrder},
            functional::{
                IntoNotificationHandlerOnceBox, IntoObservableMany, IntoObservableSingle,
                NotificationHandlerOnce,
            },
            test_util::{
                get_mock_address_1, get_mock_asset_1_arc, get_mock_asset_2_arc,
                get_mock_asset_name_1, get_mock_asset_name_2, get_mock_index_name_1,
            },
        },
        index::{
            basket::{AssetWeight, BasketDefinition},
            basket_manager::BasketNotification,
        },
        market_data::{
            market_data_connector::{
                test_util::MockMarketDataConnector, MarketDataConnector, MarketDataEvent,
            },
            order_book::order_book_manager::PricePointBookManager,
        },
        order_sender::{
            order_connector::{test_util::MockOrderConnector, OrderConnectorNotification},
            order_tracker::{OrderTracker, OrderTrackerNotification},
        },
        server::server::{test_util::MockServer, ServerEvent, ServerResponse},
        solver::{index_quote_manager::test_util::MockQuoteRequestManager, position::LotId},
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

    struct MockOrderIdProvider {
        order_ids: VecDeque<OrderId>,
        batch_order_ids: VecDeque<BatchOrderId>,
    }

    impl OrderIdProvider for MockOrderIdProvider {
        fn next_order_id(&mut self) -> OrderId {
            self.order_ids.pop_front().expect("No more Order Ids")
        }
        fn next_batch_order_id(&mut self) -> BatchOrderId {
            self.batch_order_ids
                .pop_front()
                .expect("No more BatchOrder Ids")
        }
    }

    /// Test that solver system is sane
    ///
    /// Step 1.
    ///     - Send prices for assets (top of the book and last trade)
    ///     - Send book updates for assets (top two levels)
    ///     - Emit CuratorWeightsSet event from ChainConnector mock
    ///         - Solver should respond with updating baskets
    ///         - BasketManager should confirm basket updates
    ///     - Emit NewOrder event from FIX server mock
    ///         - Solver should receive NewIndexOrder event
    /// Tick 1.
    ///     - Solver engages with new index orders:
    ///         - Fetching prices and liquidity
    ///         - Calculating contribution
    ///         - Engaging in orders with IndexOrderManager
    ///             - Solver should receive EngageIndexOrder event from IndexOrderManager
    ///         - Solver will not send order batches yet
    /// Tick 2.
    ///     - Solver will not engage with no orders (no new orders)
    ///     - Solver sends out new order batches:
    ///         - Orders from the batch will reach OrderConnector, and we fill those orders
    ///         - Solver should receive OpenLot event from InventoryManager
    ///
    /// TODO:
    ///   Solver should redistribute any suitable quantity from inventory
    ///   accorting to contribution, and notify IndexOrderManager about filled
    ///   index orders
    ///
    #[test]
    fn sbe_solver() {
        let tolerance = dec!(0.0001);
        let fund_wait_period = TimeDelta::new(600, 0).unwrap();

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

        let order_id_provider = Arc::new(RwLock::new(MockOrderIdProvider {
            order_ids: VecDeque::from_iter(
                ["Order01", "Order02", "Order03", "Order04"]
                    .into_iter()
                    .map_into(),
            ),
            batch_order_ids: VecDeque::from_iter(["Batch01", "Batch02"].into_iter().map_into()),
        }));

        let solver = Arc::new(Solver::new(
            chain_connector.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            basket_manager.clone(),
            price_tracker.clone(),
            order_book_manager.clone(),
            inventory_manager.clone(),
            order_id_provider.clone(),
            4,
            tolerance,
            fund_wait_period,
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

        let lot_ids = RwLock::new(VecDeque::<LotId>::from([
            "Lot01".into(),
            "Lot02".into(),
            "Lof03".into(),
            "Lot04".into(),
        ]));
        let order_connector_weak = Arc::downgrade(&order_connector);
        let (defer_1, deferred) = unbounded::<Box<dyn FnOnce() + Send + Sync>>();
        order_connector
            .write()
            .implementor
            .set_observer_fn(move |e: Arc<SingleOrder>| {
                let order_connector = order_connector_weak.upgrade().unwrap();
                let lot_id = lot_ids.write().pop_front().unwrap();
                let p1 = e.price
                    * match e.side {
                        Side::Buy => dec!(0.995),
                        Side::Sell => dec!(1.005),
                    };
                let p2 = e.price
                    * match e.side {
                        Side::Buy => dec!(0.998),
                        Side::Sell => dec!(1.002),
                    };
                let q1 = e.quantity * dec!(0.8);
                let q2 = e.quantity * dec!(0.2);
                let defer = defer_1.clone();
                // Note we defer first fill to make sure we don't get dead-lock
                defer_1
                    .send(Box::new(move || {
                        order_connector.write().notify_fill(
                            e.order_id.clone(),
                            lot_id.clone(),
                            e.symbol.clone(),
                            e.side,
                            p1,
                            q1,
                            dec!(0.001) * p1 * q1,
                            e.created_timestamp,
                        );
                        // We defer second fill, so that fills of different orders
                        // will be interleaved. We do that to test progressive fill-rate
                        // of the Index Order in our simulation.
                        defer
                            .send(Box::new(move || {
                                order_connector.write().notify_fill(
                                    e.order_id.clone(),
                                    lot_id.clone(),
                                    e.symbol.clone(),
                                    e.side,
                                    p2,
                                    q2,
                                    dec!(0.001) * p2 * q2,
                                    e.created_timestamp,
                                );
                            }))
                            .unwrap();
                    }))
                    .unwrap();
            });

        let (mock_chain_sender, mock_chain_receiver) = unbounded::<MockChainInternalNotification>();
        let (mock_fix_sender, mock_fix_receiver) = unbounded::<ServerResponse>();

        chain_connector
            .write()
            .implementor
            .set_observer_fn(move |response| {
                match &response {
                    MockChainInternalNotification::SolverWeightsSet(symbol, _) => {
                        println!("Solver Weights Set: {}", symbol);
                    }
                    MockChainInternalNotification::MintIndex {
                        symbol,
                        quantity,
                        receipient,
                        execution_price,
                        execution_time,
                    } => {
                        println!(
                            "Minted Index: {:5} Quantity: {:0.5} User: {} @{:0.5} {}",
                            symbol, quantity, receipient, execution_price, execution_time
                        );
                    }
                    MockChainInternalNotification::BurnIndex {
                        symbol,
                        quantity,
                        receipient,
                    } => {
                        todo!()
                    }
                    MockChainInternalNotification::Withdraw {
                        receipient,
                        amount,
                        execution_price,
                        execution_time,
                    } => {
                        todo!()
                    }
                };
                mock_chain_sender
                    .send(response)
                    .expect("Failed to send chain response");
                println!("Chain response sent.");
            });

        fix_server
            .write()
            .implementor
            .set_observer_fn(move |response| {
                match &response {
                    ServerResponse::NewIndexOrderAck {
                        address,
                        client_order_id,
                        timestamp,
                    } => {
                        println!(
                            "FIX Response: {} {} {}",
                            address, client_order_id, timestamp
                        );
                    }
                    ServerResponse::IndexOrderFill {
                        address,
                        client_order_id,
                        filled_quantity,
                        quantity_remaining,
                        timestamp,
                    } => {
                        println!(
                            "FIX Response: {} {} {:0.5} {:0.5} {}",
                            address,
                            client_order_id,
                            filled_quantity,
                            quantity_remaining,
                            timestamp
                        );
                    }
                };
                mock_fix_sender
                    .send(response)
                    .expect("Failed to send FIX response");
                println!("FIX response sent.");
            });

        let (solver_tick_sender, solver_tick_receiver) = unbounded::<DateTime<Utc>>();
        let solver_tick = |msg| solver_tick_sender.send(msg).unwrap();

        let flush_events = move || {
            // Simple dispatch loop
            loop {
                select! {
                    recv(index_order_receiver) -> res => solver.handle_index_order(res.unwrap()),
                    recv(quote_request_receiver) -> res => solver.handle_quote_request(res.unwrap()),
                    recv(price_receiver) -> res => solver.handle_price_event(res.unwrap()),
                    recv(book_receiver) -> res => solver.handle_book_event(res.unwrap()),
                    recv(basket_receiver) -> res => solver.handle_basket_event(res.unwrap()),

                    recv(chain_receiver) -> res => solver.handle_chain_event(res.unwrap())
                        .expect("Failed to handle chain event"),

                    recv(inventory_receiver) -> res => solver.handle_inventory_event(res.unwrap())
                        .expect("Failed to handle inventory event"),

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
                    recv(deferred) -> res => (res.unwrap())(),
                    recv(solver_tick_receiver) -> res => {
                        solver.solve(res.unwrap())
                    },
                    default => { break; },
                }
            }
        };

        let mut timestamp = Utc::now();

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
            dec!(90.0),
            dec!(100.0),
            dec!(10.0),
            dec!(20.0),
        );

        market_data_connector.write().notify_top_of_book(
            get_mock_asset_name_2(),
            dec!(295.0),
            dec!(300.0),
            dec!(80.0),
            dec!(50.0),
        );

        // last trade
        market_data_connector
            .write()
            .notify_trade(get_mock_asset_name_1(), dec!(90.0), dec!(5.0));

        market_data_connector.write().notify_trade(
            get_mock_asset_name_2(),
            dec!(300.0),
            dec!(15.0),
        );

        // book depth
        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_1(),
            vec![
                PricePointEntry {
                    price: dec!(90.0),
                    quantity: dec!(10.0),
                },
                PricePointEntry {
                    price: dec!(80.0),
                    quantity: dec!(40.0),
                },
            ],
            vec![
                PricePointEntry {
                    price: dec!(100.0),
                    quantity: dec!(20.0),
                },
                PricePointEntry {
                    price: dec!(110.0),
                    quantity: dec!(30.0),
                },
            ],
        );

        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_2(),
            vec![
                PricePointEntry {
                    price: dec!(295.0),
                    quantity: dec!(80.0),
                },
                PricePointEntry {
                    price: dec!(290.0),
                    quantity: dec!(100.0),
                },
            ],
            vec![
                PricePointEntry {
                    price: dec!(300.0),
                    quantity: dec!(50.0),
                },
                PricePointEntry {
                    price: dec!(305.0),
                    quantity: dec!(150.0),
                },
            ],
        );

        // necessary to wait until all market data events are ingested
        flush_events();

        // define basket
        let basket_definition = BasketDefinition::try_new(vec![
            AssetWeight::new(get_mock_asset_1_arc(), dec!(0.8)),
            AssetWeight::new(get_mock_asset_2_arc(), dec!(0.2)),
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

        print!("Chain response received.");

        chain_connector.write().notify_deposit(
            get_mock_address_1(),
            "Pay001".into(),
            dec!(1005.0) * dec!(2.5) * (Amount::ONE + dec!(0.001)),
            timestamp,
        );

        flush_events();

        println!("We sent deposit");
        solver_tick(timestamp);

        flush_events();

        fix_server
            .write()
            .notify_server_event(Arc::new(ServerEvent::NewIndexOrder {
                address: get_mock_address_1(),
                client_order_id: "Order01".into(),
                payment_id: "Pay001".into(),
                symbol: get_mock_index_name_1(),
                side: Side::Buy,
                price: dec!(1005.0),
                price_threshold: dec!(0.05),
                quantity: dec!(2.5),
                timestamp,
            }));

        flush_events();

        println!("We sent NewOrderSingle FIX message");
        solver_tick(timestamp);

        flush_events();

        println!("IndexOrderManager responded to EngageOrders");
        solver_tick(timestamp);

        flush_events();

        let fix_response = mock_fix_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive ServerResponse");

        assert!(matches!(
            fix_response,
            ServerResponse::NewIndexOrderAck {
                address: _,
                client_order_id: _,
                timestamp: _
            }
        ));

        let order1 = order_tracker_2.read().get_order(&"Order01".into());
        let order2 = order_tracker_2.read().get_order(&"Order02".into());

        assert!(matches!(order1, Some(_)));
        assert!(matches!(order2, Some(_)));
        let order1 = order1.unwrap();
        let order2 = order2.unwrap();

        assert_eq!(order1.symbol, get_mock_asset_name_1());
        assert_eq!(order2.symbol, get_mock_asset_name_2());
        assert_eq!(order1.side, Side::Buy);
        assert_eq!(order2.side, Side::Buy);

        assert_decimal_approx_eq!(order1.price, dec!(97.15), tolerance);
        assert_decimal_approx_eq!(order1.quantity, dec!(20.0), tolerance);
        assert_decimal_approx_eq!(order2.price, dec!(298.4076923), tolerance);
        assert_decimal_approx_eq!(order2.quantity, dec!(1.627806563), tolerance);

        flush_events();

        for _ in 0..2 {
            let fix_response = mock_fix_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("Failed to receive ServerResponse");

            assert!(matches!(
                fix_response,
                ServerResponse::IndexOrderFill {
                    address: _,
                    client_order_id: _,
                    filled_quantity: _,
                    quantity_remaining: _,
                    timestamp: _
                }
            ));

            println!("FIX response received.");
        }

        println!("Solver re-inserts IndexOrder from filled batch");
        solver_tick(timestamp);

        flush_events();

        println!("Solver sends next batch");
        solver_tick(timestamp);

        flush_events();

        for _ in 0..2 {
            let fix_response = mock_fix_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("Failed to receive ServerResponse");

            assert!(matches!(
                fix_response,
                ServerResponse::IndexOrderFill {
                    address: _,
                    client_order_id: _,
                    filled_quantity: _,
                    quantity_remaining: _,
                    timestamp: _
                }
            ));

            println!("FIX response received.");
        }

        println!("We moved clock 10 minutes forward");
        timestamp += fund_wait_period;
        solver_tick(timestamp);

        flush_events();

        // wait for solver to solve...
        let mint_index = mock_chain_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive MintIndex");

        assert!(matches!(
            mint_index,
            MockChainInternalNotification::MintIndex {
                symbol: _,
                quantity: _,
                receipient: _,
                execution_price: _,
                execution_time: _
            }
        ));

        println!("Chain response received.");

        println!("Scenario completed.")
    }
}
