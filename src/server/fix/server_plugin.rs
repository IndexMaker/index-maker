use std::{collections::HashSet, sync::Arc};

use crate::server::{
    fix::{
        messages::{FixHeader, FixTrailer, RequestBody, ResponseBody},
        requests::FixRequest,
        responses::FixResponse,
    },
    server::{ServerEvent, ServerResponse},
};
use alloy::primitives::address;
use axum_fix_server::{
    messages::{ServerResponse as AxumServerResponse, SessionId},
    plugins::{
        observer_plugin::ObserverPlugin,
        seq_num_plugin::{SeqNumPlugin, WithSeqNumPlugin},
        serde_plugin::SerdePlugin,
        user_plugin::{UserPlugin, WithUserPlugin},
    },
    server_plugin::ServerPlugin as AxumFixServerPlugin,
};
use chrono::Utc;
use eyre::{eyre, Result};
use k256::elliptic_curve::generic_array::GenericArray;
use k256::{
    ecdsa::{signature::DigestSigner, Signature, SigningKey},
    FieldBytes,
};
use sha2::{Digest, Sha256};
use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, ClientQuoteId, Side, Symbol},
    functional::{IntoObservableManyVTable, NotificationHandler},
};

// A composite plugin that can wrap other plugins and delegate functionality.
pub struct ServerPlugin {
    observer_plugin: ObserverPlugin<Arc<ServerEvent>>,
    serde_plugin: SerdePlugin<FixRequest, FixResponse>,
    seq_num_plugin: SeqNumPlugin<FixRequest, FixResponse>,
    user_plugin: UserPlugin,
}

impl ServerPlugin {
    pub fn new() -> Self {
        Self {
            observer_plugin: ObserverPlugin::new(),
            serde_plugin: SerdePlugin::new(),
            seq_num_plugin: SeqNumPlugin::new(),
            user_plugin: UserPlugin::new(),
        }
    }

    fn process_error(
        &self,
        user_id: &(u32, Address),
        error_msg: String,
        session_id: &SessionId,
    ) -> Result<String> {
        let seq_num = self.seq_num_plugin.last_received_seq_num(session_id);
        //let nak: Response = Response::create_nak(session_id, seq_num, error_msg);
        let mut nak = FixResponse::format_errors(&user_id, session_id, error_msg, seq_num);
        nak.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(nak)
    }

    fn process_ack(&self, user_id: &(u32, Address), session_id: &SessionId) -> Result<String> {
        let seq_num = self.seq_num_plugin.last_received_seq_num(session_id);
        let mut ack = FixResponse::format_ack(&user_id, session_id, seq_num);
        ack.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(ack)
    }

    /// fix_request_to_server_event
    ///
    /// Converts a FixRequest into a ServerEvent
    fn fix_request_to_server_event(&self, request: FixRequest) -> Result<ServerEvent> {
        let chain_id = request.chain_id;
        let address = request.address;
        let timestamp = Utc::now();

        let time_difference = timestamp - request.standard_header.timestamp;
        if time_difference.as_seconds_f64().abs() > 60.0f64 {
            Err(eyre!("Timestamp too far off"))?;
        }

        match request.standard_header.msg_type.as_str() {
            "NewIndexOrder" => {
                if let RequestBody::NewIndexOrderBody {
                    client_order_id,
                    symbol,
                    side,
                    amount,
                } = request.body
                {
                    let side = if side == "1"
                        || side == "b"
                        || side == "B"
                        || side == "buy"
                        || side == "Buy"
                        || side == "BUY"
                        || side == "bid"
                        || side == "Bid"
                        || side == "BID"
                    {
                        Side::Buy
                    } else if side == "2"
                        || side == "s"
                        || side == "S"
                        || side == "sell"
                        || side == "Sell"
                        || side == "SELL"
                        || side == "ask"
                        || side == "Ask"
                        || side == "ASK"
                    {
                        Side::Sell
                    } else {
                        return Err(eyre!("Invalid side value"));
                    };
                    let collateral_amount = amount
                        .parse()
                        .ok()
                        .and_then(Amount::from_f64_retain)
                        .unwrap_or(Amount::ZERO);
                    Ok(ServerEvent::NewIndexOrder {
                        chain_id,
                        address,
                        client_order_id: ClientOrderId::from(client_order_id),
                        symbol: Symbol::from(symbol),
                        side,
                        collateral_amount,
                        timestamp,
                    })
                } else {
                    Err(eyre!("Invalid body for NewIndexOrder message type"))
                }
            }
            "CancelIndexOrder" => {
                if let RequestBody::CancelIndexOrderBody {
                    client_order_id,
                    symbol,
                    amount,
                } = request.body
                {
                    let collateral_amount = amount
                        .parse()
                        .ok()
                        .and_then(Amount::from_f64_retain)
                        .unwrap_or(Amount::ZERO);
                    Ok(ServerEvent::CancelIndexOrder {
                        chain_id,
                        address,
                        client_order_id: ClientOrderId::from(client_order_id),
                        symbol: Symbol::from(symbol),
                        collateral_amount,
                        timestamp,
                    })
                } else {
                    Err(eyre!("Invalid body for CancelIndexOrder message type"))
                }
            }
            "NewQuoteRequest" => {
                if let RequestBody::NewQuoteRequestBody {
                    client_quote_id,
                    symbol,
                    side,
                    amount,
                } = request.body
                {
                    let side = if side == "1"
                        || side == "b"
                        || side == "B"
                        || side == "buy"
                        || side == "Buy"
                        || side == "BUY"
                        || side == "bid"
                        || side == "Bid"
                        || side == "BID"
                    {
                        Side::Buy
                    } else if side == "2"
                        || side == "s"
                        || side == "S"
                        || side == "sell"
                        || side == "Sell"
                        || side == "SELL"
                        || side == "ask"
                        || side == "Ask"
                        || side == "ASK"
                    {
                        Side::Sell
                    } else {
                        return Err(eyre!("Invalid side value"));
                    };
                    let collateral_amount = amount
                        .parse()
                        .ok()
                        .and_then(Amount::from_f64_retain)
                        .unwrap_or(Amount::ZERO);
                    Ok(ServerEvent::NewQuoteRequest {
                        chain_id,
                        address,
                        symbol: Symbol::from(symbol),
                        side,
                        collateral_amount,
                        timestamp,
                        client_quote_id: ClientQuoteId::from(client_quote_id),
                    })
                } else {
                    Err(eyre!("Invalid body for NewQuoteRequest message type"))
                }
            }
            "CancelQuoteRequest" => {
                if let RequestBody::CancelQuoteRequestBody {
                    client_quote_id,
                    symbol,
                } = request.body
                {
                    Ok(ServerEvent::CancelQuoteRequest {
                        chain_id,
                        address,
                        symbol: Symbol::from(symbol),
                        timestamp,
                        client_quote_id: ClientQuoteId::from(client_quote_id),
                    })
                } else {
                    Err(eyre!("Invalid body for CancelQuoteRequest message type"))
                }
            }
            "AccountToCustody" => {
                if let RequestBody::AccountToCustodyBody { .. } = request.body {
                    Ok(ServerEvent::AccountToCustody)
                } else {
                    Err(eyre!("Invalid body for AccountToCustody message type"))
                }
            }
            "CustodyToAccount" => {
                if let RequestBody::CustodyToAccountBody { .. } = request.body {
                    Ok(ServerEvent::CustodyToAccount)
                } else {
                    Err(eyre!("Invalid body for CustodyToAccount message type"))
                }
            }
            _ => Err(eyre!("Unsupported message type for ServerEvent conversion")),
        }
    }

    /// server_response_to_fix_response
    ///
    /// Converts a ServerResponse into a FixResponse
    fn server_response_to_fix_response(&self, response: ServerResponse) -> Result<FixResponse> {
        let (chain_id, address, session_id, msg_type, body) = match response {
            ServerResponse::NewIndexOrderAck {
                chain_id,
                address,
                client_order_id,
                timestamp,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "NewIndexOrder".to_string(),
                ResponseBody::NewOrderBody {
                    status: "new".to_string(),
                    client_order_id: client_order_id.as_str().to_string(),
                },
            ),
            ServerResponse::NewIndexOrderNak {
                chain_id,
                address,
                client_order_id,
                timestamp,
                reason,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "NewIndexOrder".to_string(),
                ResponseBody::NewOrderFailBody {
                    status: "rejected".to_string(),
                    client_order_id: client_order_id.as_str().to_string(),
                    reason: reason.to_string(),
                },
            ),
            ServerResponse::CancelIndexOrderAck {
                chain_id,
                address,
                client_order_id,
                timestamp,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "CancelIndexOrder".to_string(),
                ResponseBody::NewOrderBody {
                    status: "canceled".to_string(),
                    client_order_id: client_order_id.as_str().to_string(),
                },
            ),
            ServerResponse::CancelIndexOrderNak {
                chain_id,
                address,
                client_order_id,
                timestamp,
                reason,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "CancelIndexOrder".to_string(),
                ResponseBody::NewOrderFailBody {
                    status: "rejected".to_string(),
                    client_order_id: client_order_id.as_str().to_string(),
                    reason: reason.to_string(),
                },
            ),
            ServerResponse::IndexOrderFill {
                chain_id,
                address,
                client_order_id,
                filled_quantity,
                collateral_spent,
                collateral_remaining,
                fill_rate,
                status,
                timestamp,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "IndexOrderFill".to_string(),
                ResponseBody::IndexOrderFillBody {
                    client_order_id: client_order_id.as_str().to_string(),
                    filled_quantity: filled_quantity.to_string(),
                    collateral_spent: collateral_spent.to_string(),
                    collateral_remaining: collateral_remaining.to_string(),
                    fill_rate: fill_rate.to_string(),
                    status,
                },
            ),
            ServerResponse::MintInvoice {
                chain_id,
                address,
                mint_invoice,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "MintInvoice".to_string(),
                ResponseBody::MintInvoiceBody {
                    client_order_id: mint_invoice.client_order_id.to_string(),
                    payment_id: mint_invoice.payment_id.to_string(),
                    symbol: mint_invoice.symbol.to_string(),
                    total_amount: mint_invoice.total_amount.to_string(),
                    amount_paid: mint_invoice.amount_paid.to_string(),
                    amount_remaining: mint_invoice.amount_remaining.to_string(),
                    management_fee: mint_invoice.management_fee.to_string(),
                    assets_value: mint_invoice.assets_value.to_string(),
                    exchange_fee: mint_invoice.exchange_fee.to_string(),
                    fill_rate: mint_invoice.fill_rate.to_string(),
                    lots: mint_invoice.lots,
                    timestamp: mint_invoice.timestamp,
                },
            ),
            ServerResponse::NewIndexQuoteAck {
                chain_id,
                address,
                client_quote_id,
                timestamp,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "NewIndexQuote".to_string(),
                ResponseBody::IndexQuoteRequestBody {
                    status: "new".to_string(),
                    client_quote_id: client_quote_id.as_str().to_string(),
                },
            ),
            ServerResponse::NewIndexQuoteNak {
                chain_id,
                address,
                client_quote_id,
                timestamp,
                reason,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "NewIndexQuote".to_string(),
                ResponseBody::IndexQuoteRequestFailBody {
                    status: "rejected".to_string(),
                    client_quote_id: client_quote_id.as_str().to_string(),
                    reason: reason.to_string(),
                },
            ),
            ServerResponse::IndexQuoteResponse {
                chain_id,
                address,
                client_quote_id,
                quantity_possible,
                timestamp,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "IndexQuoteResponse".to_string(),
                ResponseBody::IndexQuoteBody {
                    client_quote_id: client_quote_id.as_str().to_string(),
                    quantity_possible: quantity_possible.to_string(),
                },
            ),
            ServerResponse::CancelIndexQuoteAck {
                chain_id,
                address,
                client_quote_id,
                timestamp,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "CancelIndexQuote".to_string(),
                ResponseBody::IndexQuoteRequestBody {
                    status: "canceled".to_string(),
                    client_quote_id: client_quote_id.as_str().to_string(),
                },
            ),
            ServerResponse::CancelIndexQuoteNak {
                chain_id,
                address,
                client_quote_id,
                timestamp,
                reason,
            } => (
                chain_id,
                address,
                SessionId::from("S-1"),
                "CancelIndexQuote".to_string(),
                ResponseBody::IndexQuoteRequestFailBody {
                    status: "rejected".to_string(),
                    client_quote_id: client_quote_id.as_str().to_string(),
                    reason: reason.to_string(),
                },
            ),
            _ => {
                return Err(eyre!(
                    "Unsupported ServerResponse type for FixResponse conversion"
                ))
            }
        };

        let mut header = FixHeader::new(msg_type);
        header.add_sender("SERVER".to_string()); // Adjust sender ID as per your logic
        header.add_target("CLIENT".to_string()); // Adjust target ID as per your logic
                                                 // SeqNum will be set later in process_outgoing or elsewhere if needed

        let msg_type = header.msg_type.as_str();
        let is_quote = msg_type.contains("Quote");

        let mut trailer = FixTrailer::new();

        if !is_quote {
            // Extract msg_type and id for payload
            let id = match &body {
                ResponseBody::NewOrderBody {
                    client_order_id, ..
                }
                | ResponseBody::NewOrderFailBody {
                    client_order_id, ..
                }
                | ResponseBody::IndexOrderFillBody {
                    client_order_id, ..
                }
                | ResponseBody::MintInvoiceBody {
                    client_order_id, ..
                } => client_order_id,
                ResponseBody::IndexQuoteRequestBody {
                    client_quote_id, ..
                }
                | ResponseBody::IndexQuoteRequestFailBody {
                    client_quote_id, ..
                }
                | ResponseBody::IndexQuoteBody {
                    client_quote_id, ..
                } => client_quote_id,
                _ => return Err(eyre!("Cannot extract id from response body")),
            };

            let payload = format!("{{\"msg_type\":\"{}\",\"id\":\"{}\"}}", msg_type, id);

            // hash the minimized-payload using sha256
            let mut hasher = Sha256::new();
            hasher.update(payload.as_bytes());
            let hash = hasher.finalize();

            // load server private key from command
            let priv_hex = std::env::var("SERVER_PRIVATE_KEY")
                .map_err(|_| eyre!("SERVER_PRIVATE_KEY not set"))?
                .trim_start_matches("0x")
                .to_string();

            let priv_bytes = hex::decode(priv_hex)?;
            let priv_array: [u8; 32] = priv_bytes
                .try_into()
                .map_err(|_| eyre!("Private key must be 32 bytes"))?;

            // signingKey from byte array
            let priv_ga: &FieldBytes = GenericArray::from_slice(&priv_array);
            let signing_key = SigningKey::from_bytes(priv_ga)
                .map_err(|e| eyre!("Failed to parse private key: {}", e))?;

            // sign the hash digest (I didnt use sign(), due to double hashed)
            let signature: Signature =
                signing_key.sign_digest(Sha256::new().chain_update(payload.as_bytes()));
            let signature_hex = hex::encode(signature.to_bytes()); // 64-byte r||s

            // get uncompressed pubkey (65 bytes, must start with 0x04)
            let verifying_key = signing_key.verifying_key();
            let pubkey_uncompressed = verifying_key.to_encoded_point(false);
            let pubkey_hex = hex::encode(pubkey_uncompressed.as_bytes());

            // set computed trailer
            trailer.add_public(format!("0x{}", pubkey_hex));
            trailer.add_signature(format!("0x{}", signature_hex));
        }

        Ok(FixResponse {
            session_id,
            standard_header: header,
            chain_id,
            address,
            body,
            standard_trailer: trailer,
        })
    }
}

impl IntoObservableManyVTable<Arc<ServerEvent>> for ServerPlugin {
    fn add_observer(&mut self, observer: Box<dyn NotificationHandler<Arc<ServerEvent>>>) {
        self.observer_plugin.add_observer(observer);
    }
}

impl AxumFixServerPlugin<ServerResponse> for ServerPlugin {
    fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<String> {
        match self.serde_plugin.process_incoming(message, session_id) {
            Ok(result) => {
                // verify signature before proceed anything
                match result.verify_signature() {
                    Ok(_) => {}
                    Err(_) => {
                        let user_id = result.get_user_id();
                        let err_msg = "Not authorised".to_string();
                        let nak = FixResponse::create_nak(
                            &user_id,
                            session_id,
                            result.get_seq_num(),
                            err_msg,
                        );
                        return Ok(serde_json::to_string(&nak)?);
                    }
                }
                // end verification part

                let user_id = &result.get_user_id();
                self.user_plugin.add_add_user_session(&user_id, session_id);

                let seq_num = result.get_seq_num();
                if self.seq_num_plugin.valid_seq_num(seq_num, session_id) {
                    // Convert FixRequest to ServerEvent
                    let event = self.fix_request_to_server_event(result).map_err(|e| {
                        let error_msg = self.process_error(user_id, e.to_string(), session_id);
                        eyre::eyre!(
                            error_msg.unwrap_or_else(|_| "Failed to process error".to_string())
                        )
                    })?;
                    self.observer_plugin.publish_request(&Arc::new(event));
                    self.process_ack(user_id, session_id)
                        .map_err(|e| eyre::eyre!("Process ACK failed: {}", e))
                } else {
                    let error_msg = format!(
                        "Invalid sequence number: {}; Last valid: {}",
                        seq_num,
                        self.seq_num_plugin.last_received_seq_num(session_id)
                    );
                    let error_msg = self.process_error(user_id, error_msg, session_id)?;
                    Err(eyre::eyre!(error_msg))
                }
            }
            Err(e) => {
                let user_id = &(0, address!("0x0000000000000000000000000000000000000000"));
                let error_msg = self.process_error(user_id, e.to_string(), session_id)?;
                return Err(eyre::eyre!(error_msg));
            }
        }
    }

    fn process_outgoing(&self, response: ServerResponse) -> Result<HashSet<(SessionId, String)>> {
        let mut result: HashSet<(SessionId, String)>;
        result = HashSet::new();

        let fix_response = self.server_response_to_fix_response(response)?;
        let user_id = fix_response.get_user_id();

        if let Ok(sessions) = self.user_plugin.get_user_sessions(&user_id) {
            for session in sessions {
                let mut response = fix_response.clone();
                response.set_seq_num(self.seq_num_plugin.next_seq_num(&session));

                if let Ok(message) = self.serde_plugin.process_outgoing(response) {
                    result.insert((session, message));
                } else {
                    return Err(eyre::eyre!("Cannot serialize response."));
                }
            }
        } else {
            let session_id = fix_response.get_session_id();
            let mut response = fix_response.clone();
            response.set_seq_num(self.seq_num_plugin.next_seq_num(&session_id.clone()));

            if let Ok(message) = self.serde_plugin.process_outgoing(response) {
                result.insert((session_id.clone(), message));
            } else {
                return Err(eyre::eyre!("Cannot serialize response."));
            }
        }
        return Ok(result);
    }

    fn create_session(&self, session_id: &SessionId) -> Result<()> {
        self.seq_num_plugin.create_session(session_id)
    }

    fn destroy_session(&self, session_id: &SessionId) -> Result<()> {
        self.user_plugin.remove_session(session_id);
        self.seq_num_plugin.destroy_session(session_id)
    }
}
