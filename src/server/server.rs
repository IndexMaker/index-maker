use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

use derive_with_baggage::WithBaggage;
use opentelemetry::propagation::Injector;
use symm_core::core::telemetry::{TracingData, WithBaggage};

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
pub enum NewQuoteRequestNakReason {
    #[error("Duplicate client quote ID: {detail:?}")]
    DuplicateClientQuoteId { detail: String },

    #[error("Invalid symbol: {detail:?}")]
    InvalidSymbol { detail: String },

    #[error("Other reason: {detail:?}")]
    OtherReason { detail: String },
}

#[derive(Error, Debug, WithBaggage)]
pub enum ServerResponse {
    // ──────── Orders ────────
    #[error("NewIndexOrder: ACK [{chain_id}:{address}] {client_order_id} {timestamp}")]
    NewIndexOrderAck {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
    },

    #[error("NewIndexOrder: NAK [{chain_id}:{address}] {client_order_id} {timestamp}: {reason:?}")]
    NewIndexOrderNak {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_order_id: ClientOrderId,
        reason: ServerResponseReason<NewIndexOrderNakReason>,
        timestamp: DateTime<Utc>,
    },

    #[error("CancelIndexOrder: ACK [{chain_id}:{address}] {client_order_id} {timestamp}")]
    CancelIndexOrderAck {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
    },

    #[error(
        "CancelIndexOrder: NAK [{chain_id}:{address}] {client_order_id} {timestamp}: {reason:?}"
    )]
    CancelIndexOrderNak {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_order_id: ClientOrderId,
        reason: ServerResponseReason<CancelIndexOrderNakReason>,
        timestamp: DateTime<Utc>,
    },

    /// Order filled (partial or full)
    #[error(
        "IndexOrderFill: [{chain_id}:{address}] {client_order_id} {timestamp} status={status}"
    )]
    IndexOrderFill {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_order_id: ClientOrderId,
        filled_quantity: Amount,
        collateral_spent: Amount,
        collateral_remaining: Amount,
        fill_rate: Amount,
        status: String,
        timestamp: DateTime<Utc>,
    },

    /// Mint invoice issued for an order
    #[error("MintInvoice: [{chain_id}:{address}] {client_order_id} {timestamp}")]
    MintInvoice {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_order_id: ClientOrderId,
        mint_invoice: MintInvoice,
        timestamp: DateTime<Utc>,
    },

    // ──────── Quotes ────────
    #[error("NewIndexQuote: ACK [{chain_id}:{address}] {client_quote_id} {timestamp}")]
    NewIndexQuoteAck {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_quote_id: ClientQuoteId,
        timestamp: DateTime<Utc>,
    },

    #[error("NewIndexQuote: NAK [{chain_id}:{address}] {client_quote_id} {timestamp}: {reason:?}")]
    NewIndexQuoteNak {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_quote_id: ClientQuoteId,
        reason: ServerResponseReason<NewIndexQuoteNakReason>,
        timestamp: DateTime<Utc>,
    },

    #[error("NewQuoteRequest: ACK [{chain_id}:{address}] {client_quote_id} {timestamp}")]
    NewQuoteRequestAck {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_quote_id: ClientQuoteId,
        timestamp: DateTime<Utc>,
    },

    #[error(
        "NewQuoteRequest: NAK [{chain_id}:{address}] {client_quote_id} {timestamp}: {reason:?}"
    )]
    NewQuoteRequestNak {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_quote_id: ClientQuoteId,
        reason: ServerResponseReason<NewQuoteRequestNakReason>,
        timestamp: DateTime<Utc>,
    },

    #[error("CancelIndexQuote: ACK [{chain_id}:{address}] {client_quote_id} {timestamp}")]
    CancelIndexQuoteAck {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_quote_id: ClientQuoteId,
        timestamp: DateTime<Utc>,
    },

    #[error(
        "CancelIndexQuote: NAK [{chain_id}:{address}] {client_quote_id} {timestamp}: {reason:?}"
    )]
    CancelIndexQuoteNak {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_quote_id: ClientQuoteId,
        reason: ServerResponseReason<CancelIndexQuoteNakReason>,
        timestamp: DateTime<Utc>,
    },

    /// Quote response (with max executable size)
    #[error("IndexQuoteResponse: [{chain_id}:{address}] {client_quote_id} {timestamp}")]
    IndexQuoteResponse {
        #[baggage]
        chain_id: u32,
        #[baggage]
        address: Address,
        #[baggage]
        client_quote_id: ClientQuoteId,
        quantity_possible: Amount,
        timestamp: DateTime<Utc>,
    },
}

impl ServerResponse {
    /// Returns (chain_id, address, client_order_id, client_quote_id).
    /// Clone non-Copy IDs to avoid moving out of &self.
    pub fn telemetry_ids(
        &self,
    ) -> (Option<u32>, Option<Address>, Option<ClientOrderId>, Option<ClientQuoteId>) {
        match self {
            // ── Order-side variants (carry client_order_id)
            ServerResponse::NewIndexOrderAck {
                chain_id,
                address,
                client_order_id,
                ..
            }
            | ServerResponse::NewIndexOrderNak {
                chain_id,
                address,
                client_order_id,
                ..
            }
            | ServerResponse::CancelIndexOrderAck {
                chain_id,
                address,
                client_order_id,
                ..
            }
            | ServerResponse::CancelIndexOrderNak {
                chain_id,
                address,
                client_order_id,
                ..
            }
            | ServerResponse::IndexOrderFill {
                chain_id,
                address,
                client_order_id,
                ..
            } => (
                Some(*chain_id),
                Some(address.clone()),
                Some(client_order_id.clone()),
                None,
            ),

            // MintInvoice carries the order id inside the payload
            ServerResponse::MintInvoice {
                chain_id,
                address,
                mint_invoice,
                ..
            } => (
                Some(*chain_id),
                Some(address.clone()),
                Some(mint_invoice.client_order_id.clone()),
                None,
            ),

            // ── Quote-side variants (carry client_quote_id)
            // Support both naming schemes: NewIndexQuote* and NewQuoteRequest*
            ServerResponse::NewIndexQuoteAck {
                chain_id,
                address,
                client_quote_id,
                ..
            }
            | ServerResponse::NewIndexQuoteNak {
                chain_id,
                address,
                client_quote_id,
                ..
            }
            | ServerResponse::NewQuoteRequestAck {
                chain_id,
                address,
                client_quote_id,
                ..
            }
            | ServerResponse::NewQuoteRequestNak {
                chain_id,
                address,
                client_quote_id,
                ..
            }
            | ServerResponse::CancelIndexQuoteAck {
                chain_id,
                address,
                client_quote_id,
                ..
            }
            | ServerResponse::CancelIndexQuoteNak {
                chain_id,
                address,
                client_quote_id,
                ..
            }
            | ServerResponse::IndexQuoteResponse {
                chain_id,
                address,
                client_quote_id,
                ..
            } => (
                Some(*chain_id),
                Some(address.clone()),
                None,
                Some(client_quote_id.clone()),
            ),
        }
    }
}
pub trait Server: IntoObservableManyVTable<Arc<ServerEvent>> + Send + Sync {
    /// Provide methods for sending FIX responses
    fn respond_with(&mut self, response: ServerResponse);
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
