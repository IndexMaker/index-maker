use binance_spot_connector_rust::http::Credentials;
use futures_util::SinkExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{handshake::client::Response, protocol::Message, Error},
    MaybeTlsStream, WebSocketStream,
};
use uuid::Uuid;

pub struct BinanceWebSocketClient;

impl BinanceWebSocketClient {
    pub async fn connect_async(
        url: &str,
        recv_window: Option<u32>,
        credentials: Option<Credentials>,
    ) -> Result<(WebSocketState<MaybeTlsStream<TcpStream>>, Response), Error> {
        let (socket, response) = connect_async(url).await?;

        println!("Connected to {}", url);
        println!("Response HTTP code: {}", response.status());
        println!("Response headers:");
        for (ref header, _value) in response.headers() {
            println!("* {}", header);
        }

        Ok((WebSocketState::new(socket, recv_window, credentials), response))
    }
}

pub struct WebSocketState<T> {
    socket: WebSocketStream<T>,
    recv_window: Option<u32>,
    credentials: Option<Credentials>,
}

impl<T: AsyncRead + AsyncWrite + Unpin> WebSocketState<T> {
    pub fn new(
        socket: WebSocketStream<T>,
        recv_window: Option<u32>,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            socket,
            recv_window,
            credentials,
        }
    }

    pub async fn send(&mut self, method: &str, params: impl IntoIterator<Item = &str>) -> String {
        let mut params_str: String = params
            .into_iter()
            .map(|param| format!("\"{}\"", param))
            .collect::<Vec<String>>()
            .join(",");

        if !params_str.is_empty() {
            params_str = format!("\"params\": [{params}],", params = params_str)
        };

        let id = Uuid::new_v4().to_string();

        let s = format!(
            "{{\"method\":\"{method}\",{params}\"id\":{id}}}",
            method = method,
            params = params_str,
            id = id
        );
        let message = Message::Text(s);
        println!("Sent {}", message);

        self.socket.send(message).await.unwrap();

        id
    }

    pub async fn close(mut self) -> Result<(), Error> {
        self.socket.close(None).await
    }
}

impl<T> From<WebSocketState<T>> for WebSocketStream<T> {
    fn from(conn: WebSocketState<T>) -> WebSocketStream<T> {
        conn.socket
    }
}

impl<T> AsMut<WebSocketStream<T>> for WebSocketState<T> {
    fn as_mut(&mut self) -> &mut WebSocketStream<T> {
        &mut self.socket
    }
}
