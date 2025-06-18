use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
    usize,
};

use eyre::{eyre, Report, Result};
use index_maker::core::functional::{MultiObserver, PublishMany};
use itertools::Itertools;
use tokio::{
    select,
    sync::{
        mpsc::{channel, Receiver, Sender},
        RwLock,
    },
};

pub enum MyServerRequest {
    NewOrder,
    CancelOrder,
}

impl ServerRequest for MyServerRequest {
    fn deserialize_from_fix(message: FixMessage) -> Result<Self, Report> {
        // process message
        match message.0.as_str() {
            "NewOrder" => Ok(MyServerRequest::NewOrder),
            "CancelOrder" => Ok(MyServerRequest::CancelOrder),
            _ => Err(eyre!("Oh no!")),
        }
    }
}

pub enum MyServerResponse {
    NewOrderAck,
    NewOrderNak,
    CancelAck,
    CancelNak,
}

impl ServerResponse for MyServerResponse {
    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, Report> {
        todo!()
    }

    fn get_session_id(&self) -> &SessionId {
        todo!()
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
    fn deserialize_from_fix(message: FixMessage) -> Result<Self, Report>;
}

pub trait ServerResponse {
    fn get_session_id(&self) -> &SessionId;
    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, Report>;
}

struct Session<Q>
where
    Q: ServerResponse,
{
    response_tx: Sender<Q>,
}

impl<Q> Session<Q>
where
    Q: ServerResponse,
{
    pub fn new(tx: Sender<Q>) -> Self {
        Self { response_tx: tx }
    }

    pub async fn send_response(&self, response: Q) -> Result<()> {
        self.response_tx
            .send(response)
            .await
            .map_err(|err| eyre!("Error {:?}", err))
    }
}

struct Server<R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    observer: MultiObserver<R>,
    sessions: HashMap<SessionId, Arc<RwLock<Session<Q>>>>,
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
        }
    }

    pub async fn send_response(&self, response: Q) -> Result<()> {
        let session_id = response.get_session_id();
        match self.sessions.get(session_id) {
            Some(session) => session.write().await.send_response(response).await,
            None => Err(eyre!("Oh, no!")),
        }
    }

    pub fn handle_server_message(&self, request: R) {
        self.observer.publish_many(&request);
    }

    pub fn create_session(&mut self) -> Result<(Arc<RwLock<Session<Q>>>, Receiver<Q>)> {
        let session_id = SessionId("123".to_owned());
        match self.sessions.entry(session_id) {
            Entry::Occupied(_) => Err(eyre!("Oh, no!")),
            Entry::Vacant(vacant_entry) => {
                let (tx, rx) = channel(1024);
                let session = vacant_entry.insert(Arc::new(RwLock::new(Session::<Q>::new(tx))));
                Ok((session.clone(), rx))
            }
        }
    }

    pub fn build_fix_message(&self, q: Q) -> Result<FixMessage> {
        let builder = FixMessageBuilder::new();
        let message = q.serialize_into_fix(builder);
        message
    }
}

fn main() {
    let server = Arc::new(RwLock::new(
        Server::<MyServerRequest, MyServerResponse>::new(),
    ));
}

async fn ws_handler<R, Q>(server: Arc<RwLock<Server<R, Q>>>) -> Result<()>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    let (session, mut receiver) = server.write().await.create_session()?;

    loop {
        select! {
            // quit => { break; }
            // s = ws.recv() => {
            // let m = FixMessage::from(s);
            // let event = R::deserialize_from_fix(m);
            // session.write().await.handle_server_message(event);
            //}
            Some(r) = receiver.recv() => {
                let fix_message = server.write().await.build_fix_message(r);
                // ws.send(fix_message.to_string());
            }
        }
    }

    Ok(())
}
