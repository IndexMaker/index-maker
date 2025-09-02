use std::sync::Arc;

use binance_sdk::common::websocket::WebsocketStream;
use binance_sdk::models::{self, WebsocketApiRateLimit};
use binance_sdk::spot::websocket_api::{
    ExchangeInfoParams, OrderPlaceParams, OrderPlaceSideEnum, OrderPlaceTimeInForceEnum,
    PingParams, UserDataStreamEventsResponse, UserDataStreamStartParams,
    UserDataStreamSubscribeParams, WebsocketApi,
};
use binance_sdk::spot::{self, websocket_api};
use binance_sdk::{config::ConfigurationWebsocketApi, spot::websocket_api::SessionLogonParams};
use chrono::{Duration, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use safe_math::safe;
use symm_core::core::bits::{Amount, SingleOrder};
use symm_core::core::decimal_ext::DecimalExt;
use symm_core::{
    core::{
        bits::{OrderId, Side, Symbol},
        functional::{PublishSingle, SingleObserver},
        limit::{LimiterConfig, MultiLimiter},
    },
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use tokio::time::sleep;
use tracing::info;

use crate::command::Command;
use crate::credentials::{ConfigureBinanceUsingCredentials, Credentials};
use crate::session_error::SessionError;
use crate::trading_markets::TradingMarkets;

pub struct TradingSessionBuilder;

impl TradingSessionBuilder {
    pub async fn build(credentials: &Credentials) -> Result<TradingSession, SessionError> {
        let configuration = ConfigurationWebsocketApi::builder()
            .configure(credentials)
            .map_err(|err| SessionError::AuthenticationError {
                message: format!("Failed to configure with credentials: {:?}", err),
            })?
            .build()
            .map_err(|err| SessionError::BadRequest {
                message: format!("Failed to build configuration: {:?}", err),
            })?;

        let client = spot::SpotWsApi::production(configuration);
        let wsapi = client.connect().await.map_err(|err| {
            SessionError::from_eyre(&eyre::eyre!("Failed to connect to Binance: {:?}", err))
        })?;

        let trading_enabled = credentials.should_enable_trading();

        Ok(TradingSession::new(
            credentials.into_session_id(),
            wsapi,
            trading_enabled,
        ))
    }
}

pub struct TradingSession {
    session_id: SessionId,
    wsapi: WebsocketApi,
    markets: TradingMarkets,
    order_limit: MultiLimiter,
    trading_enabled: bool,
}

impl TradingSession {
    fn new(session_id: SessionId, wsapi: WebsocketApi, trading_enabled: bool) -> Self {
        Self {
            session_id,
            wsapi,
            markets: TradingMarkets::new(),
            order_limit: MultiLimiter::new(vec![]),
            trading_enabled,
        }
    }

    async fn will_send_order(&mut self) -> Result<()> {
        if !self.order_limit.try_consume(1, Utc::now()) {
            let sleep_time = self
                .order_limit
                .waiting_period_half_smallest_limit(Utc::now())
                .as_seconds_f64();

            tracing::info!("Rate limit reached. Must wait for: {}s", sleep_time);

            sleep(std::time::Duration::from_secs_f64(sleep_time)).await;

            if !self.order_limit.try_consume(1, Utc::now()) {
                Err(eyre!("Failed to satisfy rate-limit"))?
            }
        }
        Ok(())
    }

    fn update_limits(&mut self, limits: &Vec<WebsocketApiRateLimit>) {
        let timestamp = Utc::now();
        let values = limits
            .iter()
            .filter(|limit| matches!(limit.rate_limit_type, models::RateLimitType::Orders))
            .map(|limit| {
                let conf = LimiterConfig::new(
                    limit.limit as usize,
                    match limit.interval {
                        models::Interval::Second => Duration::seconds(limit.interval_num.into()),
                        models::Interval::Minute => Duration::minutes(limit.interval_num.into()),
                        models::Interval::Hour => Duration::hours(limit.interval_num.into()),
                        models::Interval::Day => Duration::days(limit.interval_num.into()),
                    },
                );
                (conf, limit.count as usize)
            })
            .collect_vec();

        self.order_limit.refit(values, timestamp);
    }

    pub fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub async fn logon(&mut self) -> Result<(), SessionError> {
        let logon_params =
            SessionLogonParams::builder()
                .build()
                .map_err(|err| SessionError::BadRequest {
                    message: format!("Failed to build logon request: {}", err),
                })?;

        let res = self
            .wsapi
            .session_logon(logon_params)
            .await
            .map_err(|err| SessionError::from_eyre(&eyre::eyre!("Failed to logon: {:?}", err)))?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        Ok(())
    }

    pub async fn enable_trading(&mut self, enable: bool) -> Result<(), SessionError> {
        if self.trading_enabled == enable {
            if self.trading_enabled {
                return Err(SessionError::TradingRestriction {
                    message: String::from("Trading already enabled"),
                });
            } else {
                return Err(SessionError::TradingRestriction {
                    message: String::from("Trading already disabled"),
                });
            }
        }
        // TODO: should we await any pending orders to complete, or cancel them?
        self.trading_enabled = enable;
        tracing::info!("Trading {}", if enable { "enabled" } else { "disabled" });
        Ok(())
    }

    pub async fn new_order_single(
        &mut self,
        single_order: Arc<SingleOrder>,
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<()> {
        let side = match single_order.side {
            Side::Buy => OrderPlaceSideEnum::Buy,
            Side::Sell => OrderPlaceSideEnum::Sell,
        };

        let mut price = single_order.price;
        let mut quantity = single_order.quantity;

        // Ensure that price, quantity, and nominal are meeting the required minimum
        let allow_pad = true;

        self.markets.treat_price_quantity(
            &single_order.symbol,
            &mut price,
            &mut quantity,
            allow_pad,
        )?;

        observer
            .read()
            .publish_single(OrderConnectorNotification::NewOrder {
                order_id: single_order.order_id.clone(),
                symbol: single_order.symbol.clone(),
                side: single_order.side,
                price,
                quantity,
                timestamp: Utc::now(),
            });

        let params = OrderPlaceParams {
            id: None,
            side,
            symbol: single_order.symbol.to_string(),
            r#type: websocket_api::OrderPlaceTypeEnum::Limit,
            time_in_force: Some(OrderPlaceTimeInForceEnum::Ioc),
            price: Some(price),
            quantity: Some(quantity),
            quote_order_qty: None,
            new_client_order_id: Some(single_order.order_id.cloned()),
            new_order_resp_type: None,
            stop_price: None,
            trailing_delta: None,
            iceberg_qty: None,
            strategy_id: None,
            strategy_type: None,
            self_trade_prevention_mode: None,
            recv_window: Some(10000),
        };

        tracing::debug!("PlaceOrder: {:#?}", params);

        if self.trading_enabled {
            self.will_send_order().await?;

            let res = self
                .wsapi
                .order_place(params)
                .await
                .map_err(|err| eyre!("Failed to send order: {:?}", err))?;

            if let Some(limits) = &res.rate_limits {
                self.update_limits(limits);
            }

            let res = res
                .data()
                .map_err(|err| eyre!("Failed to place order: {:?}", err))?;

            tracing::debug!("PlaceOrder returned: {:#?}", res);
        } else {
            tracing::warn!(
                "PlaceOrder: TRADING DISABLED: Must enable trading before sending orders"
            );
        }

        Ok(())
    }

    pub async fn get_exchange_info(&mut self, symbols: Vec<Symbol>) -> Result<(), SessionError> {
        let symbols = symbols.into_iter().map(|x| x.to_string()).collect_vec();

        info!(
            "Requesting exchange information for: {}",
            symbols.join(", ")
        );

        let params = ExchangeInfoParams::builder()
            .symbols(symbols)
            .build()
            .map_err(|err| SessionError::BadRequest {
                message: format!("Failed to build exchange info params: {}", err),
            })?;

        let res = self.wsapi.exchange_info(params).await.map_err(|err| {
            SessionError::from_eyre(&eyre::eyre!("Failed to obtain exchange info: {:?}", err))
        })?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        let exchange_info = res.data().map_err(|err| SessionError::ServerError {
            message: format!("Failed to obtain exchange info data: {}", err),
        })?;

        self.markets
            .ingest_exchange_info(exchange_info)
            .map_err(|err| SessionError::BadRequest {
                message: format!("Failed to ingest exchange info: {:?}", err),
            })?;

        Ok(())
    }

    pub async fn send_command(
        &mut self,
        command: Command,
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<(), SessionError> {
        match command {
            Command::EnableTrading(enable) => self.enable_trading(enable).await,
            Command::NewOrder(single_order) => {
                if !self.trading_enabled {
                    return Err(SessionError::TradingRestriction {
                        message: String::from("Trading is disabled"),
                    });
                }

                if let Err(err) = self.new_order_single(single_order.clone(), observer).await {
                    observer
                        .read()
                        .publish_single(OrderConnectorNotification::Rejected {
                            order_id: single_order.order_id.clone(),
                            symbol: single_order.symbol.clone(),
                            side: single_order.side,
                            price: single_order.price,
                            quantity: single_order.quantity,
                            reason: format!("Failed to send order: {:?}", err),
                            timestamp: Utc::now(),
                        });

                    return Err(SessionError::from_eyre(&err));
                }
                Ok(())
            }
            Command::GetExchangeInfo(symbols) => self.get_exchange_info(symbols).await,
        }
    }

    pub async fn get_user_data(&mut self) -> Result<TradingUserData, SessionError> {
        let user_data_stream_start_params =
            UserDataStreamStartParams::builder()
                .build()
                .map_err(|err| SessionError::BadRequest {
                    message: format!("Failed to configure user data stream: {}", err),
                })?;

        let res = self
            .wsapi
            .user_data_stream_start(user_data_stream_start_params)
            .await
            .map_err(|err| {
                SessionError::from_eyre(&eyre::eyre!("Failed to start user data stream: {:?}", err))
            })?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        tracing::debug!("Start user data: {:#?}", res.data());

        let user_data_stream_subscribe_params = UserDataStreamSubscribeParams::builder()
            .build()
            .map_err(|err| SessionError::BadRequest {
                message: format!("Failed to configure user data subscription: {}", err),
            })?;

        let (res, stream) = self
            .wsapi
            .user_data_stream_subscribe(user_data_stream_subscribe_params)
            .await
            .map_err(|err| {
                SessionError::from_eyre(&eyre::eyre!(
                    "Failed to subscribe to user data stream: {:?}",
                    err
                ))
            })?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        tracing::debug!("Subscribe user data: {:#?}", res.data());

        Ok(TradingUserData::new(stream))
    }

    /// Check if the WebSocket connection is healthy
    pub async fn is_connection_healthy(&self) -> bool {
        self.wsapi.is_connected().await
    }

    /// Attempt to ping the WebSocket connection
    pub async fn ping_connection(&self) -> Result<()> {
        tracing::trace!(
            "Pinging WebSocket connection for session {}",
            self.session_id
        );

        match self.wsapi.ping(PingParams::default()).await {
            Ok(_) => {
                tracing::trace!(
                    "WebSocket connection is healthy for session {}",
                    self.session_id
                );
                Ok(())
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to ping WebSocket connection for session {}: {:?}",
                    self.session_id,
                    err
                );
                Err(eyre!("WebSocket ping failed: {:?}", err))
            }
        }
    }
}

pub struct TradingUserData {
    stream: Arc<WebsocketStream<UserDataStreamEventsResponse>>,
}

impl TradingUserData {
    fn new(stream: Arc<WebsocketStream<UserDataStreamEventsResponse>>) -> Self {
        Self { stream }
    }

    pub async fn unsubscribe(&self) {
        self.stream.unsubscribe().await;
    }

    pub fn subscribe(&self, observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>) {
        self.stream.on_message(move |data| {
            tracing::debug!("User data: {:#?}", data);

            let execution_report = match data {
                UserDataStreamEventsResponse::ExecutionReport(x) => x,
                _ => {
                    tracing::debug!("Skipping user data: Not an execution report");
                    return;
                }
            };

            let result = || -> Result<()> {
                let side = match execution_report
                    .s_uppercase
                    .ok_or_eyre("Missing side")?
                    .as_str()
                {
                    "BUY" => Side::Buy,
                    "SELL" => Side::Sell,
                    _ => Err(eyre!("Cannot parse side",))?,
                };

                let order_id =
                    OrderId::from(execution_report.c.ok_or_eyre("Missing client order ID")?);

                let symbol = Symbol::from(execution_report.s.ok_or_eyre("Missing symbol")?);

                let order_quantity = execution_report
                    .q
                    .and_then(|v| Some(Amount::from_str_exact(&v)))
                    .ok_or_eyre("Missing order quantity")?
                    .or_else(|e| Err(eyre!("Failed to parse order quantity: {:?}", e)))?;

                let cumulative_quantity = execution_report
                    .z
                    .and_then(|v| Some(Amount::from_str_exact(&v)))
                    .ok_or_eyre("Missing cumulative quantity")?
                    .or_else(|e| Err(eyre!("Failed to parse cumulative quantity: {:?}", e)))?;

                let remaining_quantity = safe!(order_quantity - cumulative_quantity)
                    .ok_or_eyre("Cannot compute remaining quantity")?;

                let executed_price = execution_report
                    .l_uppercase
                    .and_then(|v| Some(Amount::from_str_exact(&v)))
                    .ok_or_eyre("Missing executed price")?
                    .or_else(|e| Err(eyre!("Failed to parse executed price: {:?}", e)))?;

                let executed_quantity = execution_report
                    .l
                    .and_then(|v| Some(Amount::from_str_exact(&v)))
                    .ok_or_eyre("Missing executed quantity")?
                    .or_else(|e| Err(eyre!("Failed to parse executed quantity: {:?}", e)))?;

                let commission_amount = execution_report
                    .n
                    .and_then(|v| Some(Amount::from_str_exact(&v)))
                    .ok_or_eyre("Missing commission amount")?
                    .or_else(|e| Err(eyre!("Failed to parse commission amount: {:?}", e)))?;

                let commission_asset = execution_report.n_uppercase;

                let order_status = execution_report
                    .x_uppercase
                    .ok_or_eyre("Missing order status")?;

                let lot_id = execution_report
                    .t
                    .ok_or_eyre("Missing trade ID")?
                    .to_string()
                    .into();

                match order_status.as_str() {
                    "NEW" => {}
                    "FILLED" => observer
                        .read()
                        .publish_single(OrderConnectorNotification::Fill {
                            order_id: order_id.clone(),
                            lot_id,
                            symbol: symbol.clone(),
                            side,
                            price: executed_price,
                            quantity: executed_quantity,
                            fee: Amount::ZERO,
                            timestamp: Utc::now(),
                        }),
                    "PARTIALLY_FILLED" => {
                        observer
                            .read()
                            .publish_single(OrderConnectorNotification::Fill {
                                order_id,
                                lot_id,
                                symbol,
                                side,
                                price: executed_price,
                                quantity: executed_quantity,
                                fee: Amount::ZERO,
                                timestamp: Utc::now(),
                            })
                    }
                    "EXPIRED" => {
                        observer
                            .read()
                            .publish_single(OrderConnectorNotification::Cancel {
                                order_id,
                                symbol,
                                side,
                                quantity: remaining_quantity,
                                timestamp: Utc::now(),
                            })
                    }
                    other => {
                        // Note: We're firing IOC, so we can either get Fill or Cancel
                        tracing::warn!("Unsupported execution report type: {:#?}", other);
                    }
                };

                Ok(())
            };

            if let Err(err) = result() {
                tracing::warn!("Cannot parse user data: {:?}", err);
            }
        });
    }
}
