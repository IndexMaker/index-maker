use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

use symm_core::core::telemetry::{TracingData, WithBaggage};
use derive_with_baggage::WithBaggage;
use opentelemetry::propagation::Injector;

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, ClientQuoteId, Side, Symbol},
    functional::IntoObservableManyVTable,
};

use crate::solver::mint_invoice::MintInvoice;

#[derive(Serialize, WithBaggage)]
pub enum ServerEvent {
    NewIndexOrder {
        #[baggage]
        chain_id: u32,
        
        #[baggage]
        address: Address,
        
        #[baggage]
        client_order_id: ClientOrderId,

        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    },
    CancelIndexOrder {
        #[baggage]
        chain_id: u32,
        
        #[baggage]
        address: Address,
        
        #[baggage]
        client_order_id: ClientOrderId,

        symbol: Symbol,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    },
    NewQuoteRequest {
        #[baggage]
        chain_id: u32,
        
        #[baggage]
        address: Address,
        
        #[baggage]
        client_quote_id: ClientQuoteId,

        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    },
    CancelQuoteRequest {
        #[baggage]
        chain_id: u32,
        
        #[baggage]
        address: Address,
        
        #[baggage]
        client_quote_id: ClientQuoteId,

        symbol: Symbol,
        timestamp: DateTime<Utc>,
    },
    QuoteSubscribe {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        #[baggage]
        symbol: Symbol,

        timestamp: DateTime<Utc>,
    },
    QuoteUnsubscribe {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        #[baggage]
        symbol: Symbol,

        reason: String,
        timestamp: DateTime<Utc>,
    },
    AccountToCustody,
    CustodyToAccount,
}

#[derive(Error, Debug)]
pub enum NewIndexOrderNakReason {
    #[error("Duplicate client order ID: {detail:?}")]
    DuplicateClientOrderId { detail: String },

    #[error("Invalid symbol: {detail:?}")]
    InvalidSymbol { detail: String },
    
    #[error("Other reason: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug)]
pub enum CancelIndexOrderNakReason {
    #[error("Index order not found: {detail:?}")]
    IndexOrderNotFound { detail: String },
    #[error("Other reason: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug)]
pub enum NewIndexQuoteNakReason {
    #[error("Duplicate client quote ID: {detail:?}")]
    DuplicateIndexQuoteId { detail: String },

    #[error("Invalid symbol: {detail:?}")]
    InvalidSymbol { detail: String },
    // #[error("Rate-limit error: {detail:?}")]
    // TODO: RateLimitError { detail: String},
    // ^^^ probably not an error that belongs to ServerError, and most likely this needs to
    //     be handled internally by FIX server
    #[error("Other reason: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug)]
pub enum CancelIndexQuoteNakReason {
    #[error("Quote not found: {detail:?}")]
    IndexQuoteNotFound { detail: String },

    #[error("Invalid symbol: {detail:?}")]
    InvalidSymbol { detail: String },

    #[error("Other reason: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug)]
pub enum NewQuoteSubscriptionNakReason {
    #[error("Quote not found: {detail:?}")]
    IndexQuoteNotFound { detail: String },

    #[error("Invalid symbol: {detail:?}")]
    InvalidSymbol { detail: String },

    #[error("Other reason: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug)]
pub enum CancelQuoteSubscriptionNakReason {
    #[error("Quote not found: {detail:?}")]
    IndexQuoteNotFound { detail: String },

    #[error("Invalid symbol: {detail:?}")]
    InvalidSymbol { detail: String },

    #[error("Other reason: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug)]
pub enum ServerError {
    // #[error("Sequence number out of order: {detail:?}")]
    // TODO: SequenceNumberOutOfOrder { detail: String }, //< example of known server errors
    // ^^^ probably not an error that belongs to ServerError, and most likely this needs to
    //     be handled internally by FIX server
    #[error("Server Error: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug)]
pub enum ServerResponseReason<T> {
    #[error("{0:?}")]
    User(T),
    #[error("{0:?}")]
    Server(ServerError),
}

#[derive(Error, Debug)]
pub enum ServerResponse {
    #[error("NewIndexOrder: ACK [{chain_id}:{address}] {client_order_id} {timestamp}")]
    NewIndexOrderAck {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
    },
    #[error("NewIndexOrder: NAK [{chain_id}:{address}] {client_order_id} {timestamp}: {reason:?}")]
    NewIndexOrderNak {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        reason: ServerResponseReason<NewIndexOrderNakReason>,
        timestamp: DateTime<Utc>,
    },
    #[error("CancelIndexOrder: ACK [{chain_id}:{address}] {client_order_id} {timestamp}")]
    CancelIndexOrderAck {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
    },
    #[error(
        "CancelIndexOrder: NAK [{chain_id}:{address}] {client_order_id} {timestamp}: {reason:?}"
    )]
    CancelIndexOrderNak {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        reason: ServerResponseReason<CancelIndexOrderNakReason>,
        timestamp: DateTime<Utc>,
    },
    #[error("IndexOrderFill: [{chain_id}:{address}] {client_order_id} {timestamp}: {filled_quantity} {collateral_spent} {collateral_remaining}")]
    IndexOrderFill {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        filled_quantity: Amount,
        collateral_spent: Amount,
        collateral_remaining: Amount,
        timestamp: DateTime<Utc>,
    },
    #[error("MintInvoice: NAK [{chain_id}:{address}]")]
    MintInvoice {
        chain_id: u32,
        address: Address,
        mint_invoice: MintInvoice,
    },
    #[error("NewIndexQuote: ACK [{chain_id}:{address}] {client_quote_id} {timestamp}")]
    NewIndexQuoteAck {
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        timestamp: DateTime<Utc>,
    },
    #[error("NewIndexQuote: NAK [{chain_id}:{address}] {client_quote_id} {timestamp}: {reason:?}")]
    NewIndexQuoteNak {
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        reason: ServerResponseReason<NewIndexQuoteNakReason>,
        timestamp: DateTime<Utc>,
    },
    #[error("IndexOrderResponse: [{chain_id}:{address}] {client_quote_id} {timestamp}: {quantity_possible}")]
    IndexQuoteResponse {
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        quantity_possible: Amount,
        timestamp: DateTime<Utc>,
    },
    #[error("CancelIndexQuote: ACK [{chain_id}:{address}] {client_quote_id} {timestamp}")]
    CancelIndexQuoteAck {
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        timestamp: DateTime<Utc>,
    },
    #[error(
        "CancelIndexQuote: NAK [{chain_id}:{address}] {client_quote_id} {timestamp}: {reason:?}"
    )]
    CancelIndexQuoteNak {
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        reason: ServerResponseReason<CancelIndexQuoteNakReason>,
        timestamp: DateTime<Utc>,
    },
    #[error("NewQuoteSubscriptionAck: ACK [{chain_id}:{address}] {symbol} {timestamp}")]
    NewQuoteSubscriptionAck {
        chain_id: u32,
        address: Address,
        symbol: Symbol,
        timestamp: DateTime<Utc>,
    },
    #[error(
        "NewQuoteSubscriptionNak: NAK [{chain_id}:{address}] {symbol} {timestamp}: {reason:?}"
    )]
    NewQuoteSubscriptionNak {
        chain_id: u32,
        address: Address,
        symbol: Symbol,
        reason: ServerResponseReason<NewQuoteSubscriptionNakReason>,
        timestamp: DateTime<Utc>,
    },
    #[error("CancelQuoteSubscriptionAck: ACK [{chain_id}:{address}] {symbol} {timestamp}")]
    CancelQuoteSubscriptionAck {
        chain_id: u32,
        address: Address,
        symbol: Symbol,
        timestamp: DateTime<Utc>,
    },
    #[error(
        "CancelQuoteSubscriptionAck: NAK [{chain_id}:{address}] {symbol} {timestamp}: {reason:?}"
    )]
    CancelQuoteSubscriptionNak {
        chain_id: u32,
        address: Address,
        symbol: Symbol,
        reason: ServerResponseReason<CancelQuoteSubscriptionNakReason>,
        timestamp: DateTime<Utc>,
    },
}

pub trait Server: IntoObservableManyVTable<Arc<ServerEvent>> + Send + Sync {
    /// Provide methods for sending FIX responses
    fn respond_with(&mut self, response: ServerResponse);

    /// Publish a server event
    fn publish_event(&mut self, event: &Arc<ServerEvent>);
}

pub mod test_util {

    use std::sync::Arc;

    use symm_core::core::functional::{
        IntoObservableMany, IntoObservableManyVTable, MultiObserver, NotificationHandler,
        PublishMany, PublishSingle, SingleObserver,
    };

    use super::{Server, ServerEvent, ServerResponse};

    pub struct MockServer {
        observer: MultiObserver<Arc<ServerEvent>>,
        pub implementor: SingleObserver<ServerResponse>,
    }

    impl MockServer {
        pub fn new() -> Self {
            Self {
                observer: MultiObserver::new(),
                implementor: SingleObserver::new(),
            }
        }

        /// Receive FIX messages from clients
        pub fn start_server() {
            todo!()
        }

        /// Notify about FIX messages
        ///
        /// Real server would parse FIX message, this one just publishes the event as-is.
        pub fn notify_server_event(&self, server_event: Arc<ServerEvent>) {
            self.observer.publish_many(&server_event);
        }
    }

    impl Server for MockServer {
        /// provide methods for sending FIX responses
        fn respond_with(&mut self, response: ServerResponse) {
            self.implementor.publish_single(response);
        }

        fn publish_event(&mut self, event: &Arc<ServerEvent>) {
            self.observer.publish_many(event);
        }
    }

    impl IntoObservableMany<Arc<ServerEvent>> for MockServer {
        fn get_multi_observer_mut(&mut self) -> &mut MultiObserver<Arc<ServerEvent>> {
            &mut self.observer
        }
    }
    impl IntoObservableManyVTable<Arc<ServerEvent>> for MockServer {
        fn add_observer(&mut self, observer: Box<dyn NotificationHandler<Arc<ServerEvent>>>) {
            self.get_multi_observer_mut().add_observer(observer);
        }
    }
}
