use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use parking_lot::RwLock;

use crate::{
    core::{
        bits::{Address, Amount, ClientQuoteId, Side, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    server::server::{
        CancelIndexQuoteNakReason, NewIndexQuoteNakReason, Server, ServerEvent, ServerResponse,
        ServerResponseReason,
    },
    solver::index_quote::IndexQuote,
};

use super::solver::SolveQuotesResult;

pub enum QuoteRequestEvent {
    NewQuoteRequest {
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    },
    CancelQuoteRequest {
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        timestamp: DateTime<Utc>,
    },
}

pub struct QuoteRequestManager {
    observer: SingleObserver<QuoteRequestEvent>,
    pub server: Arc<RwLock<dyn Server>>,
    pub quote_requests: HashMap<(u32, Address), HashMap<Symbol, Arc<RwLock<IndexQuote>>>>,
}
impl QuoteRequestManager {
    pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
        Self {
            observer: SingleObserver::new(),
            server,
            quote_requests: HashMap::new(),
        }
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
        let user_quote_requests =
            self.quote_requests
                .get(&(chain_id, address))
                .ok_or_else(|| {
                    ServerResponseReason::User(CancelIndexQuoteNakReason::IndexQuoteNotFound {
                        detail: format!("No quotes found for user {}", address),
                    })
                })?;

        let quote_request = user_quote_requests.get(&symbol).ok_or_else(|| {
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

    pub fn quotes_solved(&mut self, solved_quotes: SolveQuotesResult) -> Result<()> {
        println!("(index-order-manager) Quotes solved...");
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
