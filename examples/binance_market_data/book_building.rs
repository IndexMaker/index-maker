use std::{sync::Arc, thread::spawn, time::Duration};

use binance_market_data::binance_market_data::BinanceMarketData;
use crossbeam::{
    channel::{bounded, unbounded},
    select,
};
use index_maker::{
    core::{
        bits::{Amount, PriceType, Side},
        functional::{IntoObservableManyArc, IntoObservableSingle},
        logging::log_init,
    },
    init_log,
    market_data::{
        market_data_connector::{MarketDataConnector, MarketDataEvent},
        order_book::order_book_manager::{OrderBookEvent, OrderBookManager, PricePointBookManager},
        price_tracker::{PriceEvent, PriceTracker},
    },
};
use parking_lot::RwLock;
use rust_decimal::dec;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    init_log!();

    let mut market_data = BinanceMarketData::new(2);

    let price_tracker = Arc::new(RwLock::new(PriceTracker::new()));
    let book_manager = Arc::new(RwLock::new(PricePointBookManager::new(dec!(0.000000001))));

    let (price_tx, price_rx) = unbounded::<PriceEvent>();
    let (book_tx, book_rx) = unbounded::<OrderBookEvent>();
    let (finish_tx, finish_rx) = bounded::<()>(1);

    let price_tracker_weak = Arc::downgrade(&price_tracker);
    let book_manager_weak = Arc::downgrade(&book_manager);

    price_tracker
        .write()
        .get_single_observer_mut()
        .set_observer_from(price_tx);
    book_manager
        .write()
        .get_single_observer_mut()
        .set_observer_from(book_tx);

    let handle_order_book_event = move |e| match e {
        OrderBookEvent::UpdateError { symbol, error } => {
            tracing::warn!("Failed to apply update for {} reason: {}", symbol, error);
        }
        OrderBookEvent::BookUpdate { symbol } => {
            let mut prices = price_tracker
                .read()
                .get_prices(PriceType::BestAsk, &[symbol.clone()]);

            if !prices.missing_symbols.is_empty() {
                tracing::warn!("No prices for {}", symbol);
                return;
            }

            let (price, limit) = if let Some(price) = prices.prices.get_mut(&symbol) {
                let p = *price;
                *price = *price * dec!(1.0001);
                (p, *price)
            } else {
                (Amount::ZERO, Amount::ZERO)
            };

            let book_manager_read = book_manager.read();
            match book_manager_read.get_liquidity(Side::Sell, &prices.prices) {
                Ok(liquidity) => {
                    let liquidity = liquidity.get(&symbol).unwrap();
                    tracing::info!(
                        "Liquidity for {} at {} <= ASK <= {} available: {}",
                        symbol,
                        price,
                        limit,
                        liquidity
                    );

                    let book = book_manager_read.get_order_book(&symbol).unwrap();
                    for entry in book.get_entries(Side::Sell, 10) {
                        tracing::debug!(
                            "Entry for {}: {:0.5} {:0.5}",
                            symbol,
                            entry.price,
                            entry.quantity
                        );
                    }
                }
                Err(err) => tracing::warn!(
                    "Encountered error while getting liquidity for {} reason: {}",
                    symbol,
                    err
                ),
            }
        }
    };

    spawn(move || {
        tracing::info!("Running event loop");
        let mut price_event_count = 0;
        loop {
            select! {
                recv(price_rx) -> _ => {
                    price_event_count += 1;
                },
                recv(book_rx) -> res => {
                    tracing::info!("Order Book event (price_event_count = {})", price_event_count);
                    price_event_count = 0;
                    handle_order_book_event(res.unwrap());
                },
                recv(finish_rx) -> _ => {
                    break;
                }
            }
        }
    });

    market_data
        .get_multi_observer_arc()
        .write()
        .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
            let price_tracker = price_tracker_weak
                .upgrade()
                .expect("Failed to access price tracker");
            let book_manager = book_manager_weak
                .upgrade()
                .expect("Failed to access book manager");
            price_tracker.write().handle_market_data(e);
            book_manager.write().handle_market_data(e);
        });

    market_data.start().expect("Failed to start market data");
    market_data
        .subscribe(&["BNBUSDT".into()])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(30)).await;

    market_data
        .stop()
        .await
        .expect("Failed to stop market data");

    finish_tx.send(()).expect("Failed to send finish");
}
