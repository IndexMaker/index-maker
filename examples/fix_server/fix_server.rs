use std::{
    collections::{hash_map::Entry, HashMap},
    future::Future,
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    usize,
};

use eyre::{eyre, OptionExt, Report, Result};
use futures_util::FutureExt;
use index_maker::{
    core::{
        bits::Symbol,
        functional::{MultiObserver, NotificationHandler, PublishMany},
    },
    market_data::market_data_connector::MarketDataConnector,
};
use itertools::{Either, Itertools};
use tokio::{
    select, spawn,
    sync::{
        mpsc::{channel, unbounded_channel, Receiver, Sender, UnboundedReceiver, UnboundedSender},
        Mutex, RwLock,
    },
    task::{spawn_blocking, JoinError, JoinHandle},
};
use axum::{
    extract::{State, WebSocketUpgrade, ws::{WebSocket, Message}}, response::IntoResponse, routing::get, Router, debug_handler
};
use tracing_subscriber::{self, layer::SubscriberExt, util::SubscriberInitExt};
use std::net::SocketAddr;
use futures_util::{StreamExt, SinkExt};

/////////////////////////////////////////////////////////////////////
/// MESSAGE
/////////////////////////////////////////////////////////////////////

pub enum MyServerRequest {
    NewOrder { text: String, session_id: SessionId },
    CancelOrder { session_id: SessionId },
}

impl ServerRequest for MyServerRequest {
    fn deserialize_from_fix(message: FixMessage, session_id: SessionId) -> Result<Self, Report> {
        // process message
        match message.0.as_str() {
            s if s.starts_with("NewOrder") => Ok(MyServerRequest::NewOrder {
                text: message.0,
                session_id,
            }),
            s if s.starts_with("CancelOrder") => Ok(MyServerRequest::CancelOrder {
                session_id,
            }),
            _ => Err(eyre!("Oh no!")),
        }
    }
}

#[derive(Debug, Clone)]
pub enum MyServerResponse {
    NewOrderAck { text: String, session_id: SessionId },
    NewOrderNak { session_id: SessionId },
    CancelAck { session_id: SessionId },
    CancelNak { session_id: SessionId },
}

impl ServerResponse for MyServerResponse {
    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, Report> {
        match self {
            MyServerResponse::NewOrderAck { text, .. } => {
                Ok(FixMessage("NewOrderAck: Acknowledged ".to_string() + text))
            }
            MyServerResponse::NewOrderNak { .. } => {
                Ok(FixMessage("NewOrderNak: Rejected".to_string()))
            }
            MyServerResponse::CancelAck { .. } => {
                Ok(FixMessage("CancelAck: Cancel Acknowledged".to_string()))
            }
            MyServerResponse::CancelNak { .. } => {
                Ok(FixMessage("CancelNak: Cancel Rejected".to_string()))
            }
        }
    }

    fn get_session_id(&self) -> &SessionId {
        match self {
            MyServerResponse::NewOrderAck { session_id, .. } => session_id,
            MyServerResponse::NewOrderNak { session_id, .. } => session_id,
            MyServerResponse::CancelAck { session_id, .. } => session_id,
            MyServerResponse::CancelNak { session_id, .. } => session_id,
        }
    }
}

pub struct FixMessageBuilder;

impl FixMessageBuilder {
    pub fn new() -> Self {
        Self
    }
}

pub struct FixMessage(String);

#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct SessionId(String);

pub trait ServerRequest
where
    Self: Sized,
{
    fn deserialize_from_fix(message: FixMessage, session_id: SessionId) -> Result<Self, Report>;
}

pub trait ServerResponse {
    fn get_session_id(&self) -> &SessionId;
    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, Report>;
}


/////////////////////////////////////////////////////////////////////
/// SESSION
/////////////////////////////////////////////////////////////////////

struct Session<Q>
where
    Q: ServerResponse,
{
    response_tx: Sender<Q>,
    session_id: SessionId,
}

impl<Q> Session<Q>
where
    Q: ServerResponse,
{
    pub fn new(tx: Sender<Q>, session_id: SessionId) -> Self {
        Self { response_tx: tx, session_id }
    }

    pub async fn send_response(&self, response: Q) -> Result<()> {
        self.response_tx
            .send(response)
            .await
            .map_err(|err| eyre!("Error {:?}", err))
    }
}


/////////////////////////////////////////////////////////////////////
/// SERVER
/////////////////////////////////////////////////////////////////////


struct Server<R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    observer: MultiObserver<R>,//Option<Box<dyn Fn(&R)>>,
    sessions: HashMap<SessionId, Arc<RwLock<Session<Q>>>>,
    session_id_counter: AtomicUsize,
}

const BUFFER_SIZE: usize = 1024;

impl<R, Q> Server<R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    pub fn new() -> Self {
        Self {
            observer: MultiObserver::new(),
            sessions: HashMap::new(),
            session_id_counter: AtomicUsize::new(1),
        }
    }

    pub async fn send_response(&self, response: Q) -> Result<()> {
        let session_id = response.get_session_id().clone();
        match self.sessions.get(&session_id) {
            Some(session) => session.write().await.send_response(response).await,
            None => Err(eyre!("Oh, no!")),
        }
    }

    pub fn handle_server_message(&self, request: R) {
        self.observer.publish_many(&request);
    }

    pub fn create_session(&mut self) -> Result<(Arc<RwLock<Session<Q>>>, Receiver<Q>, SessionId)> {
        let session_id = SessionId(format!("session_{}", self.session_id_counter.fetch_add(1, Ordering::SeqCst)));
        match self.sessions.entry(session_id.clone()) {
            Entry::Occupied(_) => Err(eyre!("Oh, no!")),
            Entry::Vacant(vacant_entry) => {
                let (tx, rx) = channel(BUFFER_SIZE);
                let session = vacant_entry.insert(Arc::new(RwLock::new(Session::<Q>::new(tx, session_id.clone()))));
                Ok((session.clone(), rx, session_id))
            }
        }
    }

    pub fn build_fix_message(&self, q: Q) -> Result<FixMessage> {
        let builder = FixMessageBuilder::new();
        let message = q.serialize_into_fix(builder);
        message
    }
}



/////////////////////////////////////////////////////////////////////
/// MAIN
/////////////////////////////////////////////////////////////////////

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    


    let server = Arc::new(RwLock::new(
        Server::<MyServerRequest, MyServerResponse>::new(),
    ));


    let server_clone = Arc::clone(&server);
    tokio::spawn(async move {
        // Define a struct to hold the server reference and implement NotificationHandler
        struct RequestHandler {
            server: Arc<RwLock<Server<MyServerRequest, MyServerResponse>>>,
        }
        // Create a closure to handle notifications
        impl NotificationHandler<MyServerRequest> for RequestHandler {
            fn handle_notification(&self, request: &MyServerRequest) {
                let server = Arc::clone(&self.server); // Clone Arc to use in async block
                match request {
                    MyServerRequest::NewOrder { text, session_id } => {
                        println!("PROCESSOR: NewOrder received with text: {} for session: {:?}", text, session_id);
                        let res = MyServerResponse::NewOrderAck { text: "42".to_string(), session_id: session_id.clone() };
                        // Spawn an async task to handle the async send_response
                        tokio::spawn(async move {
                            if let Err(e) = server.read().await.send_response(res).await {
                                println!("Failed to send response: {}", e);
                            }
                        });
                    }
                    MyServerRequest::CancelOrder { session_id } => {
                        println!("PROCESSOR: CancelOrder received");
                    }
                }
            }
        }

        // Create the processor with the server reference
        let processor = Box::new(
            RequestHandler { server: server_clone.clone() }
        ) as Box<dyn NotificationHandler<MyServerRequest> + 'static>;

        // Register the processor with the MultiObserver
        server_clone.write().await.observer.add_observer(processor);
    });



    // Build our application with a WebSocket route
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(server);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::debug!("Listening on {}", addr);

    // Start the server without TLS
    axum_server::bind(addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}



async fn ws_handler<R, Q>(
    ws: WebSocketUpgrade,
    State(server): State<Arc<RwLock<Server<R, Q>>>>
) -> impl IntoResponse
where
    R: ServerRequest + Send + 'static,
    Q: ServerResponse + Send + 'static,
{
    ws.on_upgrade(move |mut ws: WebSocket| async move {
        let (session, mut receiver, session_id) = server.write().await.create_session().unwrap();
        loop {
            select! {
                // quit => { break; }
                Some(res) = ws.recv() => {
                    match res {
                        Ok(message) => {
                            if let axum::extract::ws::Message::Text(text) = message {
                                tracing::debug!("Received message: {}", text);
                                let fix_message = FixMessage(text.to_string());
                                match R::deserialize_from_fix(fix_message,session_id.clone()) {
                                    Ok(request) => {
                                        // Handle the deserialized request (e.g., pass to server)
                                        server.read().await.handle_server_message(request);
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to deserialize message: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("WebSocket error: {}", e);
                            break;
                        }
                    }
                }
                Some(res) = receiver.recv() => {
                    match server.write().await.build_fix_message(res) {
                        Ok(fix_message) => {
                            if let Err(e) = ws.send(Message::Text(fix_message.0.into())).await {
                                tracing::error!("Failed to send WebSocket message: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to build FIX message: {}", e);
                        }
                    }
                }
            }
        }
    })
}
