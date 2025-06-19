use std::sync::Arc;

use binance_sdk::common::websocket::WebsocketStream;
use binance_sdk::spot::websocket_api::{
    OrderPlaceParams, OrderPlaceSideEnum, OrderPlaceTimeInForceEnum, UserDataStreamStartParams,
    UserDataStreamSubscribeParams, WebsocketApi,
};
use binance_sdk::spot::{self, websocket_api};
use binance_sdk::{config::ConfigurationWebsocketApi, spot::websocket_api::SessionLogonParams};
use chrono::Utc;
use eyre::{eyre, Result};
use index_maker::core::bits::{OrderId, Side, Symbol};
use index_maker::core::decimal_ext::DecimalExt;
use index_maker::core::functional::{PublishSingle, SingleObserver};
use index_maker::order_sender::order_connector::{OrderConnectorNotification, SessionId};
use parking_lot::RwLock as AtomicLock;
use rust_decimal::Decimal;
use safe_math::safe;
use serde::Deserialize;
use serde_json::Value;

use crate::command::Command;
use crate::credentials::{ConfigureBinanceUsingCredentials, Credentials};

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

        Ok(TradingSession {
            session_id: credentials.into_session_id(),
            wsapi,
        })
    }
}

pub struct TradingSession {
    session_id: SessionId,
    wsapi: WebsocketApi,
}

impl TradingSession {
    pub fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub async fn logon(&self) -> Result<()> {
        let logon_params = SessionLogonParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to build logon request: {}", err))?;

        self.wsapi
            .session_logon(logon_params)
            .await
            .map_err(|err| eyre!("Failed to logon: {}", err))?;

        Ok(())
    }

    pub async fn send_command(&self, command: Command) -> Result<()> {
        match command {
            Command::NewOrder(single_order) => {
                let params = OrderPlaceParams {
                    id: None,
                    side: match single_order.side {
                        Side::Buy => OrderPlaceSideEnum::BUY,
                        Side::Sell => OrderPlaceSideEnum::SELL,
                    },
                    symbol: single_order.symbol.to_string(),
                    r#type: websocket_api::OrderPlaceTypeEnum::LIMIT,
                    time_in_force: Some(OrderPlaceTimeInForceEnum::IOC),
                    price: Some(single_order.price),
                    quantity: Some(single_order.quantity),
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

                tracing::debug!("PlaceOrder send: {:#?}", params);

                let res = self
                    .wsapi
                    .order_place(params)
                    .await
                    .map_err(|err| eyre!("Failed to send order: {:?}", err))?;

                tracing::debug!("PlaceOrder returned: {:#?}", res);
                Ok(())
            }
        }
    }

    pub async fn subscribe(
        &self,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<TradingUserData> {
        let user_data_stream_start_params = UserDataStreamStartParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to configure user data stream: {}", err))?;

        let resp = self
            .wsapi
            .user_data_stream_start(user_data_stream_start_params)
            .await
            .map_err(|err| eyre!("Failed to start user data stream: {}", err))?;

        tracing::debug!("Start user data: {:#?}", resp.data());

        let user_data_stream_subscribe_params = UserDataStreamSubscribeParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to configure user data subscription: {}", err))?;

        let (resp, stream) = self
            .wsapi
            .user_data_stream_subscribe(user_data_stream_subscribe_params)
            .await
            .map_err(|err| eyre!("Failed to subscribe to user data stream: {}", err))?;

        tracing::debug!("Subscribe user data: {:#?}", resp.data());

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
            let remaining_quantity = match safe!(execution_report.order_quantity - execution_report.cumulative_quantity) {
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
        println!("{:#?}", execution_report);
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
        println!("{:#?}", execution_report);
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
        println!("{:#?}", execution_report);
    }
}
