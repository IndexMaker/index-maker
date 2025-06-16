use std::sync::Arc;

use binance_sdk::common::websocket::WebsocketStream;
use binance_sdk::spot;
use binance_sdk::spot::websocket_api::{
    OrderPlaceParams, UserDataStreamStartParams, UserDataStreamSubscribeParams, WebsocketApi,
};
use binance_sdk::{config::ConfigurationWebsocketApi, spot::websocket_api::SessionLogonParams};
use eyre::{eyre, Result};
use index_maker::core::functional::SingleObserver;
use index_maker::order_sender::order_connector::{OrderConnectorNotification, SessionId};
use parking_lot::RwLock as AtomicLock;
use serde_json::Value;

use crate::command::Command;
use crate::session::Credentials;

pub struct TradingSessionBuilder;

impl TradingSessionBuilder {
    pub async fn build(credentials: &Credentials) -> Result<TradingSession> {
        let configuration = ConfigurationWebsocketApi::builder()
            .api_key(credentials.api_key.clone())
            .api_secret((*credentials.get_secret_fn)())
            .build()
            .map_err(move |err| eyre!("Failed to build configuration {}", err))?;

        let client = spot::SpotWsApi::production(configuration);
        let wsapi = client
            .connect()
            .await
            .map_err(move |err| eyre!("Failed to connect to Binance: {}", err))?;

        Ok(TradingSession {
            session_id: SessionId(credentials.api_key.clone()),
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
            .map_err(|err| eyre!("Failed to logon {}", err))?;

        Ok(())
    }

    pub async fn send_command(&self, command: Command) -> Result<()> {
        match command {
            Command::NewOrder(single_order) => {
                let params = OrderPlaceParams {
                    symbol: todo!(),
                    side: todo!(),
                    r#type: todo!(),
                    id: todo!(),
                    time_in_force: todo!(),
                    price: todo!(),
                    quantity: todo!(),
                    quote_order_qty: todo!(),
                    new_client_order_id: todo!(),
                    new_order_resp_type: todo!(),
                    stop_price: todo!(),
                    trailing_delta: todo!(),
                    iceberg_qty: todo!(),
                    strategy_id: todo!(),
                    strategy_type: todo!(),
                    self_trade_prevention_mode: todo!(),
                    recv_window: todo!(),
                };

                self.wsapi.order_place(params);
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
            .map_err(|err| eyre!("Failed to configure user data stream {}", err))?;

        let resp = self
            .wsapi
            .user_data_stream_start(user_data_stream_start_params)
            .await
            .map_err(|err| eyre!("Failed to start user data stream {}", err))?;

        println!("{:#?}", resp.data());

        //wsapi.subscribe_on_ws_events(|e| match e {
        //    binance_sdk::models::WebsocketEvent::Open => todo!(),
        //    binance_sdk::models::WebsocketEvent::Message(_) => todo!(),
        //    binance_sdk::models::WebsocketEvent::Error(_) => todo!(),
        //    binance_sdk::models::WebsocketEvent::Close(_, _) => todo!(),
        //    binance_sdk::models::WebsocketEvent::Ping => todo!(),
        //    binance_sdk::models::WebsocketEvent::Pong => todo!(),
        //});

        let user_data_stream_subscribe_params = UserDataStreamSubscribeParams::builder()
            .build()
            .map_err(|err| eyre!("Failed to configure user data subscription {}", err))?;

        let (resp, strm) = self
            .wsapi
            .user_data_stream_subscribe(user_data_stream_subscribe_params)
            .await
            .map_err(|err| eyre!("Failed to subscribe to user data stream {}", err))?;

        println!("{:#?}", resp.data());

        strm.on_message(|data| {
            println!("{:#?}", data);

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
