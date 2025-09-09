use axum_fix_server::{
    messages::{FixMessage, ServerResponse as AxumServerResponse, SessionId},
    plugins::{
        rate_limit_plugin::{MessageType, RateLimitKey, WithRateLimitPlugin},
        seq_num_plugin::WithSeqNumPlugin,
        user_plugin::WithUserPlugin,
    },
};
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use symm_core::core::bits::Address;

use crate::server::fix::messages::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixResponse {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    pub chain_id: u32,
    pub address: Address,
    #[serde(flatten)]
    pub body: ResponseBody,
    pub standard_trailer: FixTrailer,
}

impl FixResponse {
    pub fn create_nak(
        user_id: &(u32, Address),
        session_id: &SessionId,
        seq_num: u32,
        error_reason: String,
    ) -> Self {
        FixResponse {
            session_id: session_id.clone(),
            standard_header: FixHeader::new("NAK".to_string()),
            chain_id: user_id.0,
            address: user_id.1,
            body: ResponseBody::NAKBody {
                ref_seq_num: seq_num,
                reason: error_reason,
            },
            standard_trailer: FixTrailer::new(),
        }
    }

    pub fn create_ack(user_id: &(u32, Address), session_id: &SessionId, seq_num: u32) -> Self {
        FixResponse {
            session_id: session_id.clone(),
            standard_header: FixHeader::new("ACK".to_string()),
            chain_id: user_id.0,
            address: user_id.1,
            body: ResponseBody::ACKBody {
                ref_seq_num: seq_num,
            },
            standard_trailer: FixTrailer::new(),
        }
    }
}

impl WithSeqNumPlugin for FixResponse {
    fn get_seq_num(&self) -> u32 {
        self.standard_header.seq_num
    }

    fn set_seq_num(&mut self, seq_num: u32) {
        self.standard_header.seq_num = seq_num;
    }
}

impl WithUserPlugin for FixResponse {
    fn get_user_id(&self) -> (u32, Address) {
        (self.chain_id, self.address)
    }
}

impl WithRateLimitPlugin for FixResponse {
    fn get_rate_limit_key(&self) -> RateLimitKey {
        RateLimitKey::User(self.chain_id, self.address)
    }

    fn get_message_weight(&self) -> usize {
        // Responses typically have lower weight than requests
        match self.body {
            ResponseBody::ACKBody { .. } => 1,
            ResponseBody::NAKBody { .. } => 1,
            ResponseBody::NewOrderBody { .. }
            | ResponseBody::NewOrderFailBody { .. }
            | ResponseBody::IndexOrderFillBody { .. } => 5,
            ResponseBody::IndexQuoteRequestBody { .. }
            | ResponseBody::IndexQuoteRequestFailBody { .. }
            | ResponseBody::IndexQuoteBody { .. } => 3,
            _ => 1,
        }
    }

    fn get_message_type(&self) -> MessageType {
        match self.body {
            ResponseBody::NewOrderBody { .. }
            | ResponseBody::NewOrderFailBody { .. }
            | ResponseBody::IndexOrderFillBody { .. } => MessageType::Order,
            ResponseBody::IndexQuoteRequestBody { .. }
            | ResponseBody::IndexQuoteRequestFailBody { .. }
            | ResponseBody::IndexQuoteBody { .. } => MessageType::Quote,
            _ => MessageType::Administrative,
        }
    }

    fn get_user_id(&self) -> (u32, Address) {
        (self.chain_id, self.address)
    }
}

impl AxumServerResponse for FixResponse {
    fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn serialize_into_fix(&self) -> Result<FixMessage> {
        let json_str =
            serde_json::to_string(self).map_err(|e| eyre!("Failed to serialize message: {}", e))?;

        tracing::debug!(
            session_id = %self.session_id,
            msg_type = %self.standard_header.msg_type,
            json_data = %json_str,
            "FIX server response sent",
        );
        Ok(FixMessage(json_str.to_owned()))
    }

    fn format_errors(
        user_id: &(u32, Address),
        session_id: &SessionId,
        error_msg: String,
        ref_seq_num: u32,
    ) -> Self {
        FixResponse::create_nak(user_id, session_id, ref_seq_num, error_msg)
    }

    fn format_ack(user_id: &(u32, Address), session_id: &SessionId, ref_seq_num: u32) -> Self {
        FixResponse::create_ack(user_id, session_id, ref_seq_num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_fix_server::plugins::rate_limit_plugin::{
        MessageType, RateLimitKey, WithRateLimitPlugin,
    };
    use symm_core::core::test_util::{get_mock_address_1, get_mock_address_2};

    #[test]
    fn test_with_rate_limit_plugin_implementation() {
        let user_id = (1, get_mock_address_1());
        let session_id = SessionId::from("test_session");

        // Test ACK response
        let ack_response = FixResponse::create_ack(&user_id, &session_id, 123);

        assert_eq!(
            axum_fix_server::plugins::rate_limit_plugin::WithRateLimitPlugin::get_user_id(
                &ack_response
            ),
            user_id
        );
        assert_eq!(
            ack_response.get_rate_limit_key(),
            RateLimitKey::User(user_id.0, user_id.1)
        );
        assert_eq!(ack_response.get_message_weight(), 1);
        assert_eq!(ack_response.get_message_type(), MessageType::Administrative);

        // Test NAK response
        let nak_response = FixResponse::create_nak(
            &user_id,
            &session_id,
            124,
            "Rate limit exceeded".to_string(),
        );

        assert_eq!(
            axum_fix_server::plugins::rate_limit_plugin::WithRateLimitPlugin::get_user_id(
                &nak_response
            ),
            user_id
        );
        assert_eq!(
            nak_response.get_rate_limit_key(),
            RateLimitKey::User(user_id.0, user_id.1)
        );
        assert_eq!(nak_response.get_message_weight(), 1);
        assert_eq!(nak_response.get_message_type(), MessageType::Administrative);
    }

    #[test]
    fn test_response_message_weights() {
        let user_id = (1, get_mock_address_1());
        let session_id = SessionId::from("test_session");

        // Test different response body types and their weights
        let test_cases = vec![
            (
                ResponseBody::ACKBody { ref_seq_num: 1 },
                1,
                MessageType::Administrative,
            ),
            (
                ResponseBody::NAKBody {
                    ref_seq_num: 1,
                    reason: "Error".to_string(),
                },
                1,
                MessageType::Administrative,
            ),
            (
                ResponseBody::NewOrderBody {
                    status: "FILLED".to_string(),
                    client_order_id: "O123".to_string(),
                },
                5,
                MessageType::Order,
            ),
            (
                ResponseBody::NewOrderFailBody {
                    status: "REJECTED".to_string(),
                    client_order_id: "O124".to_string(),
                    reason: "Invalid".to_string(),
                },
                5,
                MessageType::Order,
            ),
            (
                ResponseBody::IndexOrderFillBody {
                    client_order_id: "O125".to_string(),
                    filled_quantity: "100".to_string(),
                    collateral_spent: "1000".to_string(),
                    collateral_remaining: "500".to_string(),
                    fill_rate: "10.5".to_string(),
                    status: "FILLED".to_string(),
                },
                5,
                MessageType::Order,
            ),
            (
                ResponseBody::IndexQuoteRequestBody {
                    status: "QUOTED".to_string(),
                    client_quote_id: "Q123".to_string(),
                },
                3,
                MessageType::Quote,
            ),
            (
                ResponseBody::IndexQuoteRequestFailBody {
                    status: "REJECTED".to_string(),
                    client_quote_id: "Q124".to_string(),
                    reason: "Invalid".to_string(),
                },
                3,
                MessageType::Quote,
            ),
        ];

        for (body, expected_weight, expected_type) in test_cases {
            let response = FixResponse {
                session_id: session_id.clone(),
                standard_header: FixHeader::new("TEST".to_string()),
                chain_id: user_id.0,
                address: user_id.1,
                body,
                standard_trailer: FixTrailer::new(),
            };

            assert_eq!(response.get_message_weight(), expected_weight);
            assert_eq!(response.get_message_type(), expected_type);
        }
    }

    #[test]
    fn test_response_creation_methods() {
        let user_id = (42, get_mock_address_2());
        let session_id = SessionId::from("creation_test_session");

        // Test ACK creation
        let ack = FixResponse::create_ack(&user_id, &session_id, 100);
        assert_eq!(ack.chain_id, 42);
        assert_eq!(ack.address, get_mock_address_2());
        assert_eq!(ack.session_id, session_id);
        assert_eq!(ack.standard_header.msg_type, "ACK");
        match ack.body {
            ResponseBody::ACKBody { ref_seq_num } => assert_eq!(ref_seq_num, 100),
            _ => panic!("Expected ACKBody"),
        }

        // Test NAK creation
        let nak = FixResponse::create_nak(&user_id, &session_id, 101, "Test error".to_string());
        assert_eq!(nak.chain_id, 42);
        assert_eq!(nak.address, get_mock_address_2());
        assert_eq!(nak.session_id, session_id);
        assert_eq!(nak.standard_header.msg_type, "NAK");
        match nak.body {
            ResponseBody::NAKBody {
                ref_seq_num,
                reason,
            } => {
                assert_eq!(ref_seq_num, 101);
                assert_eq!(reason, "Test error");
            }
            _ => panic!("Expected NAKBody"),
        }
    }

    #[test]
    fn test_rate_limit_key_consistency() {
        let user_id_1 = (1, get_mock_address_1());
        let user_id_2 = (2, get_mock_address_2());
        let session_id = SessionId::from("key_test_session");

        let response_1 = FixResponse::create_ack(&user_id_1, &session_id, 1);
        let response_2 = FixResponse::create_ack(&user_id_1, &session_id, 2);
        let response_3 = FixResponse::create_ack(&user_id_2, &session_id, 3);

        // Same user should have same rate limit key
        assert_eq!(
            response_1.get_rate_limit_key(),
            response_2.get_rate_limit_key()
        );

        // Different users should have different rate limit keys
        assert_ne!(
            response_1.get_rate_limit_key(),
            response_3.get_rate_limit_key()
        );

        // Verify key structure
        assert_eq!(
            response_1.get_rate_limit_key(),
            RateLimitKey::User(1, get_mock_address_1())
        );
        assert_eq!(
            response_3.get_rate_limit_key(),
            RateLimitKey::User(2, get_mock_address_2())
        );
    }
}
