use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use parking_lot::RwLock;

use derive_with_baggage::WithBaggage;
use opentelemetry::propagation::Injector;
use symm_core::core::telemetry::{TracingData, WithBaggage};

use crate::{
    server::server::{
        CancelIndexQuoteNakReason, CancelQuoteSubscriptionNakReason, NewIndexQuoteNakReason, NewQuoteSubscriptionNakReason, Server, ServerError, ServerEvent, ServerResponse, ServerResponseReason
    },
    solver::index_quote::IndexQuote,
};
use symm_core::core::{
    bits::{Address, Amount, ClientQuoteId, Side, Symbol},
    functional::{IntoObservableSingle, PublishSingle, SingleObserver},
};

use super::solver::SolveQuotesResult;

#[derive(WithBaggage)]
pub enum QuoteRequestEvent {
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

        timestamp: DateTime<Utc>,
    },
    QuoteSubscribe {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        timestamp: DateTime<Utc>,
    },
    QuoteUnsubscribe {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        reason: String,
        timestamp: DateTime<Utc>,
    },
}

pub struct QuoteRequestManager {
    observer: SingleObserver<QuoteRequestEvent>,
    index_symbols: HashSet<Symbol>,
    pub server: Arc<RwLock<dyn Server>>,
    pub quote_requests: HashMap<(u32, Address), HashMap<Symbol, Arc<RwLock<IndexQuote>>>>,
    pub quote_subscriptions: HashMap<(u32, Address), HashSet<Symbol>>,
}
impl QuoteRequestManager {
    pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
        Self {
            observer: SingleObserver::new(),
            index_symbols: HashSet::new(),
            server,
            quote_requests: HashMap::new(),
            quote_subscriptions: HashMap::new(),
        }
    }

    pub fn add_index_symbol(&mut self, symbol: Symbol) {
        self.index_symbols.insert(symbol);
    }

    pub fn remove_index_symbol(&mut self, symbol: Symbol) {
        self.index_symbols.remove(&symbol);
    }

    fn new_quote_request(
        &mut self,
        chain_id: u32,
        address: Address,
        client_quote_id: &ClientQuoteId,
        symbol: &Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<(), ServerResponseReason<NewIndexQuoteNakReason>> {
        // Temporary sell side block
        if side == Side::Sell {
            return Err(ServerResponseReason::User(
                NewIndexQuoteNakReason::OtherReason {
                    detail: "We don't support Sell yet!".to_string(),
                },
            ));
        }

        // Returns error if basket does not exist
        if !self.index_symbols.contains(symbol) {
            tracing::info!("Basket does not exist: {}", symbol);
            return Err(ServerResponseReason::User(
                NewIndexQuoteNakReason::InvalidSymbol {
                    detail: symbol.to_string(),
                },
            ));
        }

        // Create quote requests for user if not created yet
        let user_quote_requests = self
            .quote_requests
            .entry((chain_id, address))
            .or_insert_with(|| HashMap::new());

        // Create quote request if not created yet
        match user_quote_requests.entry(symbol.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(Arc::new(RwLock::new(IndexQuote::new(
                    chain_id,
                    address.clone(),
                    client_quote_id.clone(),
                    symbol.clone(),
                    side,
                    collateral_amount,
                    timestamp.clone(),
                ))));
                Ok(())
            }
            Entry::Occupied(_) => {
                // TODO: Find out what is our strategy for new quotes
                Err(ServerResponseReason::User(
                    NewIndexQuoteNakReason::OtherReason {
                        detail: format!("Quote already in process"),
                    },
                ))
            }
        }?;

        self.observer
            .publish_single(QuoteRequestEvent::NewQuoteRequest {
                chain_id,
                address,
                client_quote_id: client_quote_id.clone(),
                symbol: symbol.clone(),
                side,
                collateral_amount,
                timestamp,
            });
        Ok(())
    }

    fn cancel_quote_request(
        &mut self,
        chain_id: u32,
        address: Address,
        client_quote_id: &ClientQuoteId,
        symbol: &Symbol,
        timestamp: DateTime<Utc>,
    ) -> Result<(), ServerResponseReason<CancelIndexQuoteNakReason>> {
        // Returns error if basket does not exist
        if !self.index_symbols.contains(symbol) {
            tracing::info!("Basket does not exist: {}", symbol);
            return Err(ServerResponseReason::User(
                CancelIndexQuoteNakReason::InvalidSymbol {
                    detail: symbol.to_string(),
                },
            ));
        }

        let user_quote_requests = self
            .quote_requests
            .get_mut(&(chain_id, address))
            .ok_or_else(|| {
                ServerResponseReason::User(CancelIndexQuoteNakReason::IndexQuoteNotFound {
                    detail: format!("No quotes found for user {}", address),
                })
            })?;

        let quote_request = user_quote_requests.remove(symbol).ok_or_else(|| {
            ServerResponseReason::User(CancelIndexQuoteNakReason::IndexQuoteNotFound {
                detail: format!("No quote found for user {} for {}", address, symbol),
            })
        })?;

        if quote_request.read().client_quote_id.ne(client_quote_id) {
            Err(ServerResponseReason::User(
                CancelIndexQuoteNakReason::IndexQuoteNotFound {
                    detail: format!(
                        "Found quote found for user {} for {} with {} != {}",
                        address,
                        symbol,
                        quote_request.read().client_quote_id,
                        client_quote_id
                    ),
                },
            ))?;
        }

        self.observer
            .publish_single(QuoteRequestEvent::CancelQuoteRequest {
                chain_id,
                address,
                client_quote_id: client_quote_id.clone(),
                timestamp,
            });
        Ok(())
    }

    fn new_quote_subscription(
        &mut self,
        chain_id: u32,
        address: Address,
        symbol: &Symbol,
        timestamp: DateTime<Utc>,
    ) -> Result<(), ServerResponseReason<NewQuoteSubscriptionNakReason>> {
        // Returns error if basket does not exist
        if !self.index_symbols.contains(symbol) {
            tracing::info!("Basket does not exist: {}", symbol);
            return Err(ServerResponseReason::User(
                NewQuoteSubscriptionNakReason::InvalidSymbol {
                    detail: symbol.to_string(),
                },
            ));
        }

        // Create quote subscriptions for user if not created yet
        let user_quote_subcriptions = self
            .quote_subscriptions
            .entry((chain_id, address))
            .or_insert_with(|| HashSet::new());

        // Add subscription to HashSet if possible
        if !user_quote_subcriptions.insert(symbol.clone()) {
            return Err(ServerResponseReason::User(
                NewQuoteSubscriptionNakReason::OtherReason {
                    detail: format!("Already subscribed to quote: {}", symbol),
                },
            ));
        }

        self.observer
            .publish_single(QuoteRequestEvent::QuoteSubscribe {
                chain_id,
                address,
                timestamp,
            });
        Ok(())
    }


    fn cancel_quote_subscription(
        &mut self,
        chain_id: u32,
        address: Address,
        symbol: &Symbol,
        reason: String,
        timestamp: DateTime<Utc>,
    ) -> Result<(), ServerResponseReason<CancelQuoteSubscriptionNakReason>> {
        // Returns error if basket does not exist
        if !self.index_symbols.contains(symbol) {
            tracing::info!("Basket does not exist: {}", symbol);
            return Err(ServerResponseReason::User(
                CancelQuoteSubscriptionNakReason::InvalidSymbol {
                    detail: symbol.to_string(),
                },
            ));
        }

        // Create quote subscriptions for user if not created yet
        let user_quote_subcriptions = self
            .quote_subscriptions
            .get_mut(&(chain_id, address))
            .ok_or_else(|| {
                ServerResponseReason::User(CancelQuoteSubscriptionNakReason::IndexQuoteNotFound {
                    detail: format!("No subscription found for user {}", address),
                })
            })?;

        // Add subscription to HashSet if possible
        if !user_quote_subcriptions.remove(&symbol) {
            return Err(ServerResponseReason::User(
                CancelQuoteSubscriptionNakReason::OtherReason {
                    detail: format!("Subscribed not found: {}", symbol),
                },
            ));
        }

        self.observer
            .publish_single(QuoteRequestEvent::QuoteUnsubscribe {
                chain_id,
                address,
                timestamp,
                reason,
            });
        Ok(())
    }

    pub fn quotes_solved(&mut self, solved_quotes: SolveQuotesResult) -> Result<()> {
        //tracing::info!("(index-order-manager) Quotes solved...");

        // We need to remove solved (or failed) quotes first, before we send
        // response to FIX, because once user receives FIX response, they
        // can send new quotes.
        for quote in solved_quotes
            .solved_quotes
            .iter()
            .chain(solved_quotes.failed_quotes.iter())
        {
            let (key, symbol) = {
                let quote_read = quote.read();
                (
                    (quote_read.chain_id, quote_read.address),
                    quote_read.symbol.clone(),
                )
            };

            self.quote_requests
                .get_mut(&key)
                .and_then(|quotes| quotes.remove(&symbol));
        }

        // First send FIX responses for all solved quotes...
        for quote in solved_quotes.solved_quotes {
            let (chain_id, address, client_quote_id, quantity_possible, timestamp) = {
                let quote_read = quote.read();
                (
                    quote_read.chain_id,
                    quote_read.address,
                    quote_read.client_quote_id.clone(),
                    quote_read.quantity_possible,
                    quote_read.timestamp,
                )
            };

            self.observer
                .publish_single(QuoteRequestEvent::CancelQuoteRequest {
                    chain_id,
                    address,
                    client_quote_id: client_quote_id.clone(),
                    timestamp,
                });

            self.server
                .write()
                .respond_with(ServerResponse::IndexQuoteResponse {
                    chain_id,
                    address,
                    client_quote_id: client_quote_id,
                    quantity_possible: quantity_possible,
                    timestamp,
                });
        }

        // Then send FIX responses for any failed quotes...
        for quote in solved_quotes.failed_quotes {
            let (chain_id, address, client_quote_id, timestamp, status) = {
                let quote_read = quote.read();
                (
                    quote_read.chain_id,
                    quote_read.address,
                    quote_read.client_quote_id.clone(),
                    quote_read.timestamp,
                    quote_read.status,
                )
            };

            self.observer
                .publish_single(QuoteRequestEvent::CancelQuoteRequest {
                    chain_id,
                    address,
                    client_quote_id: client_quote_id.clone(),
                    timestamp,
                });

            self.server
                .write()
                .respond_with(ServerResponse::NewIndexQuoteNak {
                    chain_id,
                    address,
                    client_quote_id,
                    reason: ServerResponseReason::Server(ServerError::OtherReason {
                        detail: format!("Failed to compute quote: {:?}", status),
                    }),
                    timestamp,
                });
        }

        Ok(())
    }

    /// receive QR
    pub fn handle_server_message(&mut self, notification: &ServerEvent) -> Result<()> {
        match notification {
            ServerEvent::NewQuoteRequest {
                chain_id,
                address,
                client_quote_id,
                symbol,
                side,
                collateral_amount,
                timestamp,
            } => {
                if let Err(reason) = self.new_quote_request(
                    *chain_id,
                    *address,
                    client_quote_id,
                    symbol,
                    *side,
                    *collateral_amount,
                    *timestamp,
                ) {
                    let result = match &reason {
                        ServerResponseReason::User(..) => Ok(()),
                        ServerResponseReason::Server(err) => {
                            Err(eyre!("Internal server error: {:?}", err))
                        }
                    };
                    self.server
                        .write()
                        .respond_with(ServerResponse::NewIndexQuoteNak {
                            chain_id: *chain_id,
                            address: *address,
                            client_quote_id: client_quote_id.clone(),
                            reason,
                            timestamp: *timestamp,
                        });
                    result
                } else {
                    self.server
                        .write()
                        .respond_with(ServerResponse::NewIndexQuoteAck {
                            chain_id: *chain_id,
                            address: *address,
                            client_quote_id: client_quote_id.clone(),
                            timestamp: *timestamp,
                        });
                    Ok(())
                }
            }
            ServerEvent::CancelQuoteRequest {
                chain_id,
                address,
                client_quote_id,
                symbol,
                timestamp,
            } => {
                if let Err(reason) = self.cancel_quote_request(
                    *chain_id,
                    *address,
                    client_quote_id,
                    symbol,
                    *timestamp,
                ) {
                    let result = match &reason {
                        ServerResponseReason::User(..) => Ok(()),
                        ServerResponseReason::Server(err) => {
                            Err(eyre!("Internal server error: {:?}", err))
                        }
                    };
                    self.server
                        .write()
                        .respond_with(ServerResponse::CancelIndexQuoteNak {
                            chain_id: *chain_id,
                            address: *address,
                            client_quote_id: client_quote_id.clone(),
                            reason,
                            timestamp: *timestamp,
                        });
                    result
                } else {
                    self.server
                        .write()
                        .respond_with(ServerResponse::CancelIndexQuoteAck {
                            chain_id: *chain_id,
                            address: *address,
                            client_quote_id: client_quote_id.clone(),
                            timestamp: *timestamp,
                        });
                    Ok(())
                }
            }
            ServerEvent::QuoteSubscribe {
                chain_id,
                address,
                symbol,
                timestamp,
            } => {
                if let Err(reason) = self.new_quote_subscription(
                    *chain_id,
                    *address,
                    symbol,
                    *timestamp,
                ) {
                    let result = match &reason {
                        ServerResponseReason::User(..) => Ok(()),
                        ServerResponseReason::Server(err) => {
                            Err(eyre!("Internal server error: {:?}", err))
                        }
                    };
                    self.server
                        .write()
                        .respond_with(ServerResponse::NewQuoteSubscriptionNak {
                            chain_id: *chain_id,
                            address: *address,
                            symbol: symbol.clone(),
                            reason,
                            timestamp: *timestamp,
                        });
                    result
                } else {
                    self.server
                        .write()
                        .respond_with(ServerResponse::NewQuoteSubscriptionAck {
                            chain_id: *chain_id,
                            address: *address,
                            symbol: symbol.clone(),
                            timestamp: *timestamp,
                        });
                    Ok(())
                }
            },
            ServerEvent::QuoteUnsubscribe {
                chain_id,
                address,
                symbol,
                reason,
                timestamp,
            } => {
                if let Err(err_reason) = self.cancel_quote_subscription(
                    *chain_id,
                    *address,
                    symbol,
                    reason.clone(),
                    *timestamp,
                ) {
                    let result = match &err_reason {
                        ServerResponseReason::User(..) => Ok(()),
                        ServerResponseReason::Server(err) => {
                            Err(eyre!("Internal server error: {:?}", err))
                        }
                    };
                    self.server
                        .write()
                        .respond_with(ServerResponse::CancelQuoteSubscriptionNak {
                            chain_id: *chain_id,
                            address: *address,
                            symbol: symbol.clone(),
                            reason: err_reason,
                            timestamp: *timestamp,
                        });
                    result
                } else if *reason != "disconnect".to_string()  {
                    self.server
                        .write()
                        .respond_with(ServerResponse::CancelQuoteSubscriptionAck {
                            chain_id: *chain_id,
                            address: *address,
                            symbol: symbol.clone(),
                            timestamp: *timestamp,
                        });
                    Ok(())
                } else {
                    Ok(())
                }
            },
            _ => Ok(()),
        }
    }

    pub fn respond_quote(&mut self, _quote: ()) {
        todo!()
    }
}

impl IntoObservableSingle<QuoteRequestEvent> for QuoteRequestManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<QuoteRequestEvent> {
        &mut self.observer
    }
}
