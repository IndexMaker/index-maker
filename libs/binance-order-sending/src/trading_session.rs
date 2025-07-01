use std::sync::Arc;

use binance_sdk::common::websocket::WebsocketStream;
use binance_sdk::models::{self, WebsocketApiRateLimit};
use binance_sdk::spot::websocket_api::{
    ExchangeInfoParams, OrderPlaceParams, OrderPlaceSideEnum, OrderPlaceTimeInForceEnum,
    UserDataStreamStartParams, UserDataStreamSubscribeParams, WebsocketApi,
};
use binance_sdk::spot::{self, websocket_api};
use binance_sdk::{config::ConfigurationWebsocketApi, spot::websocket_api::SessionLogonParams};
use chrono::{Duration, Utc};
use eyre::{eyre, Result};
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use rust_decimal::Decimal;
use safe_math::safe;
use serde::Deserialize;
use serde_json::Value;
use symm_core::core::bits::SingleOrder;
use symm_core::{
    core::{
        bits::{OrderId, Side, Symbol},
        decimal_ext::DecimalExt,
        functional::{PublishSingle, SingleObserver},
        limit::{LimiterConfig, MultiLimiter},
    },
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use tokio::time::sleep;

use crate::command::Command;
use crate::credentials::{ConfigureBinanceUsingCredentials, Credentials};
use crate::trading_markets::TradingMarkets;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionReport {
    #[serde(rename = "E")]
    event_time: i64,
    #[serde(rename = "t")]
    trade_id: i64,
    #[serde(rename = "c")]
    client_order_id: String,
    #[serde(rename = "x")]
    execution_type: String,
    #[serde(rename = "r")]
    order_reject_reason: String,
    #[serde(rename = "X")]
    order_status: String,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "S")]
    side: String,
    #[serde(rename = "q")]
    order_quantity: Decimal,
    #[serde(rename = "p")]
    order_price: Decimal,
    #[serde(rename = "l")]
    executed_quantity: Decimal,
    #[serde(rename = "L")]
    executed_price: Decimal,
    #[serde(rename = "z")]
    cumulative_quantity: Decimal,
    #[serde(rename = "n")]
    commission_amount: Decimal,
    #[serde(rename = "N")]
    commission_asset: Option<String>,
}

pub struct TradingSessionBuilder;

impl TradingSessionBuilder {
    pub async fn build(credentials: &Credentials) -> Result<TradingSession> {
        let configuration = ConfigurationWebsocketApi::builder()
            .configure(credentials)
            .map_err(move |err| eyre!("Failed to configure with credentials: {:?}", err))?
            .build()
            .map_err(move |err| eyre!("Failed to build configuration: {:?}", err))?;

        let client = spot::SpotWsApi::production(configuration);
        let wsapi = client
            .connect()
            .await
            .map_err(move |err| eyre!("Failed to connect to Binance: {:?}", err))?;

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
                .waiting_period_half_limit(Utc::now())
                .as_seconds_f64();

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

    pub async fn logon(&mut self) -> Result<()> {
        let logon_params = SessionLogonParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to build logon request: {}", err))?;

        let res = self
            .wsapi
            .session_logon(logon_params)
            .await
            .map_err(|err| eyre!("Failed to logon: {}", err))?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        Ok(())
    }

    pub async fn enable_trading(&mut self, enable: bool) -> Result<()> {
        if self.trading_enabled == enable {
            if self.trading_enabled {
                Err(eyre!("Trading already enabled"))?;
            } else {
                Err(eyre!("Trading already disabled"))?;
            }
        }
        // TODO: should we await any pending orders to complete, or cancel them?
        self.trading_enabled = enable;
        Ok(())
    }

    pub async fn new_order_single(&mut self, single_order: Arc<SingleOrder>) -> Result<()> {
        let side = match single_order.side {
            Side::Buy => OrderPlaceSideEnum::Buy,
            Side::Sell => OrderPlaceSideEnum::Sell,
        };

        let mut price = single_order.price;
        let mut quantity = single_order.quantity;

        self.markets
            .treat_price_quantity(&single_order.symbol, &mut price, &mut quantity)?;

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

            tracing::debug!("PlaceOrder returned: {:#?}", res);
        } else {
            tracing::warn!(
                "PlaceOrder: TRADING DISABLED: Must enable trading before sending orders"
            );
        }

        Ok(())
    }

    pub async fn get_exchange_info(&mut self) -> Result<()> {
        let params = ExchangeInfoParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to build exchange info params: {}", err))?;

        let res = self
            .wsapi
            .exchange_info(params)
            .await
            .map_err(|err| eyre!("Failed to obtain exchange info: {}", err))?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        let exchange_info = res
            .data()
            .map_err(|err| eyre!("Failed to obtain exchange info data: {}", err))?;

        self.markets.ingest_exchange_info(exchange_info)?;

        Ok(())
    }

    pub async fn send_command(
        &mut self,
        command: Command,
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<()> {
        match command {
            Command::EnableTrading(enable) => self.enable_trading(enable).await,
            Command::NewOrder(single_order) => {
                if let Err(err) = self.new_order_single(single_order.clone()).await {
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

                    Err(err)?;
                }
                Ok(())
            }
            Command::GetExchangeInfo() => self.get_exchange_info().await,
        }
    }

    pub async fn subscribe(
        &mut self,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<TradingUserData> {
        let user_data_stream_start_params = UserDataStreamStartParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to configure user data stream: {}", err))?;

        let res = self
            .wsapi
            .user_data_stream_start(user_data_stream_start_params)
            .await
            .map_err(|err| eyre!("Failed to start user data stream: {}", err))?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        tracing::debug!("Start user data: {:#?}", res.data());

        let user_data_stream_subscribe_params = UserDataStreamSubscribeParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to configure user data subscription: {}", err))?;

        let (res, stream) = self
            .wsapi
            .user_data_stream_subscribe(user_data_stream_subscribe_params)
            .await
            .map_err(|err| eyre!("Failed to subscribe to user data stream: {}", err))?;

        if let Some(limits) = &res.rate_limits {
            self.update_limits(limits);
        }

        tracing::debug!("Subscribe user data: {:#?}", res.data());

        stream.on_message(move |data| {
            tracing::debug!("User data: {:#?}", data);

            let execution_report = match serde_json::from_value::<ExecutionReport>(data) {
                Ok(x) => x,
                Err(err) => {
                    tracing::warn!("Cannot parse user data: {:#?}", err);
                    return;
                }
            };

            let side = match execution_report.side.as_str() {
                "BUY" => Side::Buy,
                "SELL" => Side::Sell,
                _ => {
                    tracing::warn!("Cannot parse side in: {:?}", execution_report.side);
                    return;
                }
            };

            let order_id = OrderId::from(execution_report.client_order_id);
            let symbol = Symbol::from(execution_report.symbol);
            let remaining_quantity =
                match safe!(execution_report.order_quantity - execution_report.cumulative_quantity)
                {
                    Some(x) => x,
                    None => {
                        tracing::warn!("Cannot compute remaining quantity: Math error");
                        return;
                    }
                };

            match execution_report.order_status.as_str() {
                "FILLED" => observer
                    .read()
                    .publish_single(OrderConnectorNotification::Fill {
                        order_id,
                        lot_id: execution_report.trade_id.to_string().into(),
                        symbol,
                        side,
                        price: execution_report.executed_price,
                        quantity: execution_report.executed_quantity,
                        fee: execution_report.commission_amount,
                        timestamp: Utc::now(),
                    }),
                "EXPIRED" => observer
                    .read()
                    .publish_single(OrderConnectorNotification::Cancel {
                        order_id,
                        symbol,
                        side,
                        quantity: remaining_quantity,
                        timestamp: Utc::now(),
                    }),
                "NEW" => {
                    tracing::debug!(
                        "New order: {} {:?} {}: {} @ {}",
                        order_id,
                        side,
                        symbol,
                        execution_report.order_quantity,
                        execution_report.order_price
                    );
                }
                other => {
                    // Note: We're firing IOC, so we can either get Fill or Cancel
                    tracing::warn!("Unsupported execution report type: {:#?}", other);
                }
            }
        });

        Ok(TradingUserData::new(stream))
    }
}

pub struct TradingUserData {
    stream: Arc<WebsocketStream<Value>>,
}

impl TradingUserData {
    fn new(stream: Arc<WebsocketStream<Value>>) -> Self {
        Self { stream }
    }

    pub async fn unsubscribe(&self) {
        self.stream.unsubscribe().await;
    }
}

#[cfg(test)]
mod test {

    use serde_json::json;

    use crate::trading_session::ExecutionReport;

    #[test]
    fn execution_report_filled_test() {
        let report = json!({
            "C": json!(""),
            "E": json!(1750369643158i64),
            "F": json!("0.00000000"),
            "I": json!(1627832233),
            "L": json!("559.53000000"),
            "M": json!(true),
            "N": json!("BNB"),
            "O": json!(1750369643157i64),
            "P": json!("0.00000000"),
            "Q": json!("0.00000000"),
            "S": json!("BUY"),
            "T": json!(1750369643157i64),
            "V": json!("EXPIRE_MAKER"),
            "W": json!(1750369643157i64),
            "X": json!("FILLED"),
            "Y": json!("11.19060000"),
            "Z": json!("11.19060000"),
            "c": json!("O-1750369642968"),
            "e": json!("executionReport"),
            "f": json!("IOC"),
            "g": json!(-1),
            "i": json!(797425372i64),
            "l": json!("0.02000000"),
            "m": json!(false),
            "n": json!("0.00001425"),
            "o": json!("LIMIT"),
            "p": json!("559.60000000"),
            "q": json!("0.02000000"),
            "r": json!("NONE"),
            "s": json!("BNBEUR"),
            "t": json!(37744156i64),
            "w": json!(false),
            "x": json!("TRADE"),
            "z": json!("0.02000000"),
        });

        let execution_report = serde_json::from_value::<ExecutionReport>(report);
        assert!(matches!(execution_report, Ok(_)));
        tracing::debug!("{:#?}", execution_report);
    }

    #[test]
    fn execution_report_cancelled_test() {
        let report = json!({
            "C": json!(""),
            "E": json!(1750368429610i64),
            "F": json!("0.00000000"),
            "I": json!(1627826559),
            "L": json!("0.00000000"),
            "M": json!(false),
            "N": json!(Option::<String>::None),
            "O": json!(1750368429609i64),
            "P": json!("0.00000000"),
            "Q": json!("0.00000000"),
            "S": json!("BUY"),
            "T": json!(1750368429609i64),
            "V": json!("EXPIRE_MAKER"),
            "W": json!(1750368429609i64),
            "X": json!("EXPIRED"),
            "Y": json!("0.00000000"),
            "Z": json!("0.00000000"),
            "c": json!("O-1750368429416"),
            "e": json!("executionReport"),
            "f": json!("IOC"),
            "g": json!(-1),
            "i": json!(797422555i64),
            "l": json!("0.00000000"),
            "m": json!(false),
            "n": json!("0"),
            "o": json!("LIMIT"),
            "p": json!("559.00000000"),
            "q": json!("0.02000000"),
            "r": json!("NONE"),
            "s": json!("BNBEUR"),
            "t": json!(-1),
            "w": json!(false),
            "x": json!("EXPIRED"),
            "z": json!("0.00000000"),
        });

        let execution_report = serde_json::from_value::<ExecutionReport>(report);
        assert!(matches!(execution_report, Ok(_)));
        tracing::debug!("{:#?}", execution_report);
    }

    #[test]
    fn execution_report_open_test() {
        let report = json!({
            "C": json!(""),
            "E": json!(1750368215187i64),
            "F": json!("0.00000000"),
            "I": json!(1627825726i64),
            "L": json!("0.00000000"),
            "M": json!(false),
            "N": json!(Option::<String>::None),
            "O": json!(1750368215186i64),
            "P": json!("0.00000000"),
            "Q": json!("0.00000000"),
            "S": json!("BUY"),
            "T": json!(1750368215186i64),
            "V": json!("EXPIRE_MAKER"),
            "W": json!(1750368215186i64),
            "X": json!("NEW"),
            "Y": json!("0.00000000"),
            "Z": json!("0.00000000"),
            "c": json!("O-1750368214990"),
            "e": json!("executionReport"),
            "f": json!("GTC"),
            "g": json!(-1),
            "i": json!(797422142i64),
            "l": json!("0.00000000"),
            "m": json!(false),
            "n": json!("0"),
            "o": json!("LIMIT"),
            "p": json!("559.00000000"),
            "q": json!("0.02000000"),
            "r": json!("NONE"),
            "s": json!("BNBEUR"),
            "t": json!(-1),
            "w": json!(true),
            "x": json!("NEW"),
            "z": json!("0.00000000"),
        });

        let execution_report = serde_json::from_value::<ExecutionReport>(report);
        assert!(matches!(execution_report, Ok(_)));
        tracing::debug!("{:#?}", execution_report);
    }
}
