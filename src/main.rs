use std::{
    env,
    sync::Arc,
    thread,
};

use binance_market_data::binance_market_data::BinanceMarketData;
use binance_order_sending::{binance_order_sending::BinanceOrderSending, credentials::Credentials};
use crossbeam::{
    channel::{unbounded, Receiver},
    select,
};
use eyre::{eyre, Result};
use parking_lot::RwLock;
use rust_decimal::{dec, Decimal};
use symm_core::{
    core::{
        bits::Symbol,
        functional::{IntoObservableManyArc, IntoObservableSingle, IntoObservableSingleArc},
        logging::log_init,
    },
    init_log,
    market_data::{
        market_data_connector::{self, MarketDataConnector, MarketDataEvent},
        order_book::order_book_manager::{OrderBookEvent, PricePointBookManager},
        price_tracker::{PriceEvent, PriceTracker},
    },
    order_sender::{
        inventory_manager::{InventoryEvent, InventoryManager},
        order_connector::{OrderConnector, OrderConnectorNotification},
        order_tracker::{OrderTracker, OrderTrackerNotification},
    },
};
use tokio::{runtime::Handle, sync::watch, time::{sleep, Sleep}};

fn get_tollerance() -> Decimal {
    dec!(0.000000001)
}

async fn start_market_data(
    symbols: &[Symbol],
) -> Result<(
    Arc<RwLock<BinanceMarketData>>,
    Arc<RwLock<PriceTracker>>,
    Arc<RwLock<PricePointBookManager>>,
)> {
    let price_tracker = Arc::new(RwLock::new(PriceTracker::new()));
    let book_manager = Arc::new(RwLock::new(PricePointBookManager::new(get_tollerance())));

    let market_data = Arc::new(RwLock::new(BinanceMarketData::new(100)));

    let price_tracker_clone = price_tracker.clone();
    let book_manager_clone = book_manager.clone();

    market_data
        .write()
        .get_multi_observer_arc()
        .write()
        .add_observer_fn(move |event: &Arc<MarketDataEvent>| {
            price_tracker_clone.write().handle_market_data(&*event);
            book_manager_clone.write().handle_market_data(&*event);
        });

    market_data
        .write()
        .start()
        .map_err(|err| eyre!("Failed to start Market Data: {:?}", err))?;

    market_data
        .write()
        .subscribe(symbols)
        .map_err(|err| eyre!("Failed to subscribe for market data: {:?}", err))?;

    Ok((market_data, price_tracker, book_manager))
}

async fn start_order_sender(
    credentials: Credentials,
) -> Result<(
    Arc<RwLock<BinanceOrderSending>>,
    Receiver<OrderConnectorNotification>,
)> {
    let order_sender = Arc::new(RwLock::new(BinanceOrderSending::new()));

    let (connector_event_tx, connector_event_rx) = unbounded::<OrderConnectorNotification>();

    order_sender
        .write()
        .get_single_observer_arc()
        .write()
        .set_observer_from(connector_event_tx);

    order_sender
        .write()
        .start()
        .map_err(|err| eyre!("Failed to start order sender: {:?}", err))?;

    order_sender
        .write()
        .logon(Some(credentials))
        .map_err(|err| eyre!("Failed to logon: {:?}", err))?;

    Ok((order_sender, connector_event_rx))
}

fn build_order_tracker(
    order_connector: Arc<RwLock<dyn OrderConnector>>,
) -> Result<(
    Arc<RwLock<OrderTracker>>,
    Receiver<OrderTrackerNotification>,
)> {
    let order_tracker = Arc::new(RwLock::new(OrderTracker::new(
        order_connector,
        get_tollerance(),
    )));

    let (order_tracker_tx, order_tracker_rx) = unbounded::<OrderTrackerNotification>();

    order_tracker
        .write()
        .get_single_observer_mut()
        .set_observer_from(order_tracker_tx);

    Ok((order_tracker, order_tracker_rx))
}

fn build_inventory_manager(
    order_tracker: Arc<RwLock<OrderTracker>>,
) -> Result<(Arc<RwLock<InventoryManager>>, Receiver<InventoryEvent>)> {
    let inventory_manager = Arc::new(RwLock::new(InventoryManager::new(
        order_tracker,
        get_tollerance(),
    )));

    let (inventory_event_tx, inventory_event_rx) = unbounded::<InventoryEvent>();

    inventory_manager
        .write()
        .get_single_observer_mut()
        .set_observer_from(inventory_event_tx);

    Ok((inventory_manager, inventory_event_rx))
}

#[tokio::main]
async fn main() {
    init_log!();

    let api_key = env::var("BINANCE_API_KEY").expect("No API key in env");
    let credentials = Credentials::new(
        api_key,
        move || env::var("BINANCE_API_SECRET").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_FILE").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_PHRASE").ok(),
    );

    let symbols = [
        Symbol::from("BNBEUR"),
        Symbol::from("BTCEUR"),
        Symbol::from("ETHEUR"),
        Symbol::from("LINKEUR"),
    ];

    let (market_data, price_tracker, book_manager) = start_market_data(&symbols)
        .await
        .expect("Failed to start market data");

    let (order_sender, connector_event_rx) = start_order_sender(credentials)
        .await
        .expect("Failed to start order sender");

    let (order_tracker, order_tracker_rx) =
        build_order_tracker(order_sender.clone()).expect("Failed to build order tracker");

    let (inventory_manager, inventory_event_rx) =
        build_inventory_manager(order_tracker.clone()).expect("Failed to build inventory manager");

    price_tracker
        .write()
        .get_single_observer_mut()
        .set_observer_fn(move |e: PriceEvent| match e {
            PriceEvent::PriceChange { symbol } => tracing::debug!("PriceInfo {}", symbol),
        });

    book_manager
        .write()
        .get_single_observer_mut()
        .set_observer_fn(move |e: OrderBookEvent| match e {
            OrderBookEvent::BookUpdate { symbol } => {
                tracing::debug!("BookUpdate {}", symbol)
            }
            OrderBookEvent::UpdateError { symbol, error } => {
                tracing::warn!("BookUpdate {}: Error: {}", symbol, error)
            }
        });

    let (stop_tx, stop_rx) = unbounded::<()>();
    let (tokio_tx, mut tokio_rx) = watch::channel(false);

    thread::spawn(move || loop {
        select! {
            recv(stop_rx) -> _ => {
                tokio_tx.send(true).unwrap();
                break;
            },
            recv(connector_event_rx) -> res => {
                order_tracker.write().handle_order_notification(res.unwrap()).unwrap();
            },
            recv(order_tracker_rx) -> res => {
                inventory_manager.write().handle_fill_report(res.unwrap()).unwrap();
            },
            recv(inventory_event_rx) -> res => {
                tracing::warn!("Inventory Event unexpected: {:?}", res.unwrap());
            }
        }
    });

    sleep(std::time::Duration::from_secs(60)).await;

    stop_tx.send(()).unwrap();
    tokio_rx.wait_for(|v| *v).await.unwrap();

    order_sender.write().stop().await.unwrap();
    market_data.write().stop().await.unwrap();

    // TODO: replace unwrap() with error logging
}
