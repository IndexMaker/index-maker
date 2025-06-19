use std::sync::Arc;

use binance_sdk::common::websocket::WebsocketStream;
use binance_sdk::spot::websocket_api::{
    OrderPlaceNewOrderRespTypeEnum, OrderPlaceParams, OrderPlaceSideEnum,
    OrderPlaceTimeInForceEnum, UserDataStreamStartParams, UserDataStreamSubscribeParams,
    WebsocketApi,
};
use binance_sdk::spot::{self, websocket_api};
use binance_sdk::{config::ConfigurationWebsocketApi, spot::websocket_api::SessionLogonParams};
use eyre::{eyre, Result};
use index_maker::core::bits::Side;
use index_maker::core::functional::SingleObserver;
use index_maker::order_sender::order_connector::{OrderConnectorNotification, SessionId};
use parking_lot::RwLock as AtomicLock;
use serde_json::Value;

use crate::command::Command;
use crate::credentials::{ConfigureBinanceUsingCredentials, Credentials};

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
                    symbol: single_order.symbol.to_string(),
                    side: match single_order.side {
                        Side::Buy => OrderPlaceSideEnum::BUY,
                        Side::Sell => OrderPlaceSideEnum::SELL,
                    },
                    r#type: websocket_api::OrderPlaceTypeEnum::LIMIT,
                    id: Some(single_order.order_id.cloned()),
                    time_in_force: Some(OrderPlaceTimeInForceEnum::IOC),
                    price: Some(single_order.price.try_into()?),
                    quantity: Some(single_order.quantity.try_into()?),
                    quote_order_qty: None,
                    new_client_order_id: None,
                    new_order_resp_type: Some(OrderPlaceNewOrderRespTypeEnum::ACK),
                    stop_price: None,
                    trailing_delta: None,
                    iceberg_qty: None,
                    strategy_id: None,
                    strategy_type: None,
                    self_trade_prevention_mode: None,
                    recv_window: None,
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

        let (resp, strm) = self
            .wsapi
            .user_data_stream_subscribe(user_data_stream_subscribe_params)
            .await
            .map_err(|err| eyre!("Failed to subscribe to user data stream: {}", err))?;

        tracing::debug!("Subscribe user data: {:#?}", resp.data());

        strm.on_message(|data| {
            tracing::debug!("User data: {:#?}", data);

            //    // TODO parse message and publish
            //    observer.read().publish_single(OrderConnectorNotification::Fill {
            //        order_id: "1".into(),
            //        lot_id: "L".into(),
            //        symbol: "A".into(),
            //        side: Side::Buy,
            //        price: dec!(1.0),
            //        quantity: dec!(1.0),
            //        fee: dec!(1.0),
            //        timestamp: Utc::now() });
            //}
        });

        Ok(TradingUserData {
            observer,
            stream: strm,
        })
    }
}

pub struct TradingUserData {
    observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    stream: Arc<WebsocketStream<Value>>,
}

impl TradingUserData {
    pub async fn unsubscribe(&self) {
        self.stream.unsubscribe().await;
    }
}
