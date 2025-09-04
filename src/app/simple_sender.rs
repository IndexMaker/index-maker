use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use chrono::{Duration, Utc};
use eyre::{eyre, Context, OptionExt, Result};
use parking_lot::RwLock;
use rust_decimal::dec;
use safe_math::safe;
use symm_core::{
    core::{
        async_loop::AsyncLoop,
        bits::{Amount, Side, SingleOrder, Symbol},
        decimal_ext::DecimalExt,
        functional::{
            IntoObservableSingleVTable, NotificationHandlerOnce, OneShotPublishSingle,
            OneShotSingleObserver, PublishSingle, SingleObserver,
        },
        limit::{LimiterConfig, MultiLimiter},
        persistence::{Persist, Persistence},
    },
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification, SessionId},
};
use tokio::{
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    time::sleep,
};

pub struct SimpleOrderHandler {
    observer: Arc<RwLock<SingleObserver<OrderConnectorNotification>>>,
    balances: Arc<RwLock<HashMap<Symbol, Amount>>>,
    main_quote_currency: Symbol,
    order_limit: RwLock<MultiLimiter>,
    last_lot_number: AtomicUsize,
}

impl SimpleOrderHandler {
    fn new(
        observer: Arc<RwLock<SingleObserver<OrderConnectorNotification>>>,
        balances: Arc<RwLock<HashMap<Symbol, Amount>>>,
    ) -> Self {
        Self {
            observer,
            main_quote_currency: Symbol::from("USDC"),
            balances,
            order_limit: RwLock::new(MultiLimiter::new(vec![
                LimiterConfig::new(100, Duration::seconds(10)),
                LimiterConfig::new(6_000, Duration::minutes(1)),
                LimiterConfig::new(200_000, Duration::days(1)),
            ])),
            last_lot_number: AtomicUsize::new(0usize),
        }
    }

    async fn will_send_order(&self) -> Result<()> {
        if !self.order_limit.write().try_consume(1, Utc::now()) {
            let sleep_time = self
                .order_limit
                .read()
                .waiting_period_half_smallest_limit(Utc::now())
                .as_seconds_f64();

            tracing::info!("Rate limit reached. Must wait for: {}s", sleep_time);

            sleep(std::time::Duration::from_secs_f64(sleep_time)).await;

            if !self.order_limit.write().try_consume(1, Utc::now()) {
                Err(eyre!("Failed to satisfy rate-limit"))?
            }
        }
        Ok(())
    }

    async fn handle_new_order(&self, order: &Arc<SingleOrder>) -> Result<()> {
        self.will_send_order()
            .await
            .map_err(|err| eyre!("Failed to await rate limit: {:?}", err))?;

        // Simulate some delay to exchange
        sleep(std::time::Duration::from_millis(1)).await;

        self.observer
            .read()
            .publish_single(OrderConnectorNotification::NewOrder {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: order.side,
                price: order.price,
                quantity: order.quantity,
                timestamp: order.created_timestamp,
            });

        // Simulate some delay from exchange
        sleep(std::time::Duration::from_millis(1)).await;

        let lot_number = self.last_lot_number.fetch_add(1, Ordering::Relaxed);

        let executed_quantity = order.quantity;
        let executed_price = order.price;
        let executed_value = safe!(executed_quantity * executed_price).ok_or_eyre("Math error")?;

        let fee_rate = dec!(0.0005);
        let fee = safe!(executed_value * fee_rate).ok_or_eyre("Math error")?;
        let execution_cost = safe!(executed_value + fee).ok_or_eyre("Math error")?;

        {
            let mut balances_write = self.balances.write();
            match balances_write.entry(order.symbol.clone()) {
                Entry::Occupied(mut occupied_entry) => {
                    match order.side {
                        Side::Buy => *occupied_entry.get_mut() += executed_quantity,
                        Side::Sell => *occupied_entry.get_mut() -= executed_quantity,
                    };
                }
                Entry::Vacant(vacant_entry) => {
                    match order.side {
                        Side::Buy => vacant_entry.insert(executed_quantity),
                        Side::Sell => vacant_entry.insert(-executed_quantity),
                    };
                }
            }
            match balances_write.entry(self.main_quote_currency.clone()) {
                Entry::Occupied(mut occupied_entry) => {
                    match order.side {
                        Side::Buy => *occupied_entry.get_mut() -= execution_cost,
                        Side::Sell => *occupied_entry.get_mut() += execution_cost,
                    };
                }
                Entry::Vacant(vacant_entry) => {
                    match order.side {
                        Side::Buy => vacant_entry.insert(-execution_cost),
                        Side::Sell => vacant_entry.insert(execution_cost),
                    };
                }
            }
        }

        self.observer
            .read()
            .publish_single(OrderConnectorNotification::Fill {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: order.side,
                price: executed_price,
                quantity: executed_quantity,
                timestamp: order.created_timestamp,
                lot_id: format!("{}-L-{}", order.order_id, lot_number).into(),
                fee,
            });

        self.observer
            .read()
            .publish_single(OrderConnectorNotification::Cancel {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: order.side,
                quantity: Amount::ZERO,
                timestamp: order.created_timestamp,
            });

        Ok(())
    }

    async fn handle_get_balances(
        &self,
        observer: OneShotSingleObserver<HashMap<Symbol, Amount>>,
    ) -> Result<()> {
        observer.one_shot_publish_single((&*self.balances.read()).clone());
        Ok(())
    }
}

enum SimpleSenderCommand {
    Logon(SessionId),
    SendOrder(Arc<SimpleOrderHandler>, Arc<SingleOrder>),
    GetBalances(
        Arc<SimpleOrderHandler>,
        OneShotSingleObserver<HashMap<Symbol, Amount>>,
    ),
}

pub struct SimpleOrderSender {
    observer: Arc<RwLock<SingleObserver<OrderConnectorNotification>>>,
    persistence: Arc<dyn Persistence + Send + Sync + 'static>,
    command_loop: AsyncLoop<()>,
    command_tx: UnboundedSender<SimpleSenderCommand>,
    command_rx: Option<UnboundedReceiver<SimpleSenderCommand>>,
    sessions: Arc<RwLock<HashMap<SessionId, Arc<SimpleOrderHandler>>>>,
    balances: Arc<RwLock<HashMap<Symbol, Amount>>>,
}

impl SimpleOrderSender {
    pub fn new(persistence: Arc<dyn Persistence + Send + Sync + 'static>) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            observer: Arc::new(RwLock::new(SingleObserver::new())),
            persistence,
            command_loop: AsyncLoop::new(),
            command_tx: tx,
            command_rx: Some(rx),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            balances: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn start(&mut self) -> Result<()> {
        self.load()?;

        let balances = self.balances.clone();
        let sessions = self.sessions.clone();
        let observer = self.observer.clone();
        let mut rx = self
            .command_rx
            .take()
            .ok_or_eyre("SimpleSender already started")?;

        self.command_loop.start(async move |cancel_token| {
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(command) = rx.recv() => match command {
                        SimpleSenderCommand::Logon(session_id) => {
                            sessions.write().insert(
                                session_id.clone(),
                                Arc::new(SimpleOrderHandler::new(
                                    observer.clone(), balances.clone()
                                )),
                            );

                            observer
                                .read()
                                .publish_single(OrderConnectorNotification::SessionLogon {
                                    session_id,
                                    timestamp: Utc::now(),
                                });
                        },
                        SimpleSenderCommand::SendOrder(order_handler, single_order) => {
                            if let Err(err) = order_handler.handle_new_order(&single_order).await {
                                tracing::warn!("Failed to handle new order: {:?}", err);
                            }
                        },
                        SimpleSenderCommand::GetBalances(order_handler, observer) => {
                            if let Err(err) = order_handler.handle_get_balances(observer).await {
                                tracing::warn!("Failed to handle get balances: {:?}", err);
                            }
                        },
                    }
                }
            }
            observer
                .read()
                .publish_single(OrderConnectorNotification::SessionLogout {
                    session_id: "Session-1".into(),
                    reason: "Session ended".into(),
                    timestamp: Utc::now(),
                });
        });

        Ok(())
    }

    pub fn logon(&mut self, session_id: SessionId) -> Result<()> {
        if !self.command_rx.is_none() {
            Err(eyre!("SimpleSender wasn't started"))?;
        }

        self.command_tx
            .send(SimpleSenderCommand::Logon(session_id))
            .map_err(|err| eyre!("Failed to send logon: {:?}", err))?;

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.command_loop
            .stop()
            .await
            .map_err(|err| eyre!("Failed to stop SimpleSender: {:?}", err))?;

        self.store()?;

        Ok(())
    }
}

impl OrderConnector for SimpleOrderSender {
    fn send_order(&mut self, session_id: SessionId, order: &Arc<SingleOrder>) -> Result<()> {
        let session = self
            .sessions
            .read()
            .get(&session_id)
            .cloned()
            .ok_or_eyre("Cannot find session")?;

        self.command_tx
            .send(SimpleSenderCommand::SendOrder(session, order.clone()))
            .map_err(|err| eyre!("Failed to send order: {:?}", err))?;

        Ok(())
    }

    fn get_balances(
        &self,
        session_id: SessionId,
        observer: OneShotSingleObserver<HashMap<Symbol, Amount>>,
    ) -> Result<()> {
        let session = self
            .sessions
            .read()
            .get(&session_id)
            .cloned()
            .ok_or_eyre("Cannot find session")?;

        self.command_tx
            .send(SimpleSenderCommand::GetBalances(session, observer))
            .map_err(|err| eyre!("Failed to send get balances: {:?}", err))?;

        Ok(())
    }
}

impl Persist for SimpleOrderSender {
    fn load(&mut self) -> Result<()> {
        if let Some(value) = self.persistence.load_value()? {
            self.balances = Arc::new(RwLock::new(
                serde_json::from_value(value).context("Failed to load state")?,
            ));
        }
        Ok(())
    }

    fn store(&self) -> Result<()> {
        let balances = &*self.balances.read();
        let value = serde_json::to_value(balances).context("Failed to store state")?;
        self.persistence.store_value(value)?;
        Ok(())
    }
}

impl IntoObservableSingleVTable<OrderConnectorNotification> for SimpleOrderSender {
    fn set_observer(
        &mut self,
        observer: Box<dyn NotificationHandlerOnce<OrderConnectorNotification>>,
    ) {
        self.observer.write().set_observer(observer);
    }
}
