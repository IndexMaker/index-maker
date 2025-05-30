use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use itertools::Either;
use parking_lot::RwLock;

use crate::{
    core::{
        bits::{Address, Amount, ClientQuoteId, Side, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    server::server::{
        CancelIndexQuoteNakReason, NewIndexQuoteNakReason, Server, ServerError, ServerEvent,
        ServerResponse, ServerResponseReason,
    },
    solver::index_quote::IndexQuote,
};

pub enum QuoteRequestEvent {
    NewQuoteRequest,
    CancelQuoteRequest,
}

pub struct QuoteRequestManager {
    observer: SingleObserver<QuoteRequestEvent>,
    pub server: Arc<RwLock<dyn Server>>,
    pub quote_requests: HashMap<(u32, Address, Symbol), IndexQuote>,
}
impl QuoteRequestManager {
    pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
        Self {
            observer: SingleObserver::new(),
            server,
            quote_requests: HashMap::new(),
        }
    }

    fn notify_quote_request(&self, _quote_request: ()) {
        self.observer
            .publish_single(QuoteRequestEvent::NewQuoteRequest);
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
        let _ = collateral_amount;
        // 1. store QR
        //self.quote_requests.entry(key)
        // 2. notify about new QR
        self.notify_quote_request(());
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
                        ServerResponseReason::Server(err) => Err(eyre!("Internal server error: {:?}", err)),
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
                        ServerResponseReason::Server(err) => Err(eyre!("Internal server error: {:?}", err)),
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
