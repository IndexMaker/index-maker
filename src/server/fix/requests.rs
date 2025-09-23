use std::fmt;

use crate::server::fix::messages::*;
use alloy_primitives::Keccak256;
use axum_fix_server::{
    messages::{FixMessage, ServerRequest as AxumServerRequest, SessionId},
    plugins::{
        rate_limit_plugin::{MessageType, RateLimitKey, WithRateLimitPlugin},
        seq_num_plugin::WithSeqNumPlugin,
        user_plugin::WithUserPlugin,
    },
};
use ethers::utils::hash_message;
use eyre::{eyre, Result};
use k256::ecdsa::signature::hazmat::PrehashVerifier;

use k256::ecdsa::{Signature, VerifyingKey};
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize,
};
use symm_core::core::bits::Address;

#[derive(Serialize, Debug)]
pub struct FixRequest {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    pub chain_id: u32,
    pub address: Address,
    #[serde(flatten)]
    pub body: RequestBody,
    pub standard_trailer: Option<FixTrailer>,
}

impl WithSeqNumPlugin for FixRequest {
    fn get_seq_num(&self) -> u32 {
        self.standard_header.seq_num
    }

    fn set_seq_num(&mut self, seq_num: u32) {
        self.standard_header.seq_num = seq_num;
    }
}

impl WithUserPlugin for FixRequest {
    fn get_user_id(&self) -> (u32, Address) {
        (self.chain_id, self.address)
    }
}

impl WithRateLimitPlugin for FixRequest {
    fn get_rate_limit_key(&self) -> RateLimitKey {
        RateLimitKey::User(self.chain_id, self.address)
    }

    fn get_message_weight(&self) -> usize {
        match self.standard_header.msg_type.as_str() {
            "NewIndexOrder" => 10,
            "CancelIndexOrder" => 5,
            "NewQuoteRequest" => 5,
            "CancelQuoteRequest" => 3,
            "AccountToCustody" | "CustodyToAccount" => 1,
            _ => 1,
        }
    }

    fn get_message_type(&self) -> MessageType {
        match self.standard_header.msg_type.as_str() {
            "NewIndexOrder" | "CancelIndexOrder" => MessageType::Order,
            "NewQuoteRequest" | "CancelQuoteRequest" => MessageType::Quote,
            _ => MessageType::Administrative,
        }
    }

    fn get_user_id(&self) -> (u32, Address) {
        (self.chain_id, self.address)
    }
}

impl<'de> Deserialize<'de> for FixRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FixRequestVisitor;

        impl<'de> Visitor<'de> for FixRequestVisitor {
            type Value = FixRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct FixRequest")
            }

            fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut standard_header: Option<FixHeader> = None;
                let mut chain_id: Option<u32> = None;
                let mut address: Option<Address> = None;
                let mut standard_trailer: Option<FixTrailer> = None;
                let mut body_fields: serde_json::Map<String, serde_json::Value> =
                    serde_json::Map::new();

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "standard_header" => standard_header = Some(map.next_value()?),
                        "chain_id" => chain_id = Some(map.next_value()?),
                        "address" => address = Some(map.next_value()?),
                        "standard_trailer" => standard_trailer = Some(map.next_value()?),
                        _ => {
                            // Collect all other fields into a map for the body
                            let value: serde_json::Value = map.next_value()?;
                            body_fields.insert(key, value);
                        }
                    }
                }

                let standard_header =
                    standard_header.ok_or_else(|| de::Error::missing_field("standard_header"))?;
                let chain_id = chain_id.ok_or_else(|| de::Error::missing_field("chain_id"))?;
                let address = address.ok_or_else(|| de::Error::missing_field("address"))?;
                let standard_trailer = standard_trailer; // allow None

                let msg_type = &standard_header.msg_type;
                let body = match msg_type.as_str() {
                    "NewIndexOrder" => {
                        let client_order_id = body_fields
                            .remove("client_order_id")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("client_order_id"))?;

                        let symbol = body_fields
                            .remove("symbol")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("symbol"))?;

                        let side = body_fields
                            .remove("side")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("side"))?;

                        let amount = body_fields
                            .remove("amount")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("amount"))?;

                        RequestBody::NewIndexOrderBody {
                            client_order_id,
                            symbol,
                            side,
                            amount,
                        }
                    }
                    "CancelIndexOrder" => {
                        // Extract fields for CancelIndexOrderBody
                        let client_order_id = body_fields
                            .remove("client_order_id")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("client_order_id"))?;

                        let symbol = body_fields
                            .remove("symbol")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("symbol"))?;

                        let amount = body_fields
                            .remove("amount")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("amount"))?;

                        RequestBody::CancelIndexOrderBody {
                            client_order_id,
                            symbol,
                            amount,
                        }
                    }
                    "NewQuoteRequest" => {
                        let client_quote_id = body_fields
                            .remove("client_quote_id")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("client_quote_id"))?;

                        let symbol = body_fields
                            .remove("symbol")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("symbol"))?;

                        let side = body_fields
                            .remove("side")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("side"))?;

                        let amount = body_fields
                            .remove("amount")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("amount"))?;

                        RequestBody::NewQuoteRequestBody {
                            client_quote_id,
                            symbol,
                            side,
                            amount,
                        }
                    }
                    "CancelQuoteRequest" => {
                        let client_quote_id = body_fields
                            .remove("client_quote_id")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("client_quote_id"))?;

                        let symbol = body_fields
                            .remove("symbol")
                            .and_then(|v| serde_json::from_value::<String>(v).ok())
                            .ok_or_else(|| de::Error::missing_field("symbol"))?;

                        RequestBody::CancelQuoteRequestBody {
                            client_quote_id,
                            symbol,
                        }
                    }
                    "AccountToCustody" => serde_json::from_value(serde_json::Value::Object(
                        body_fields,
                    ))
                    .map_err(|e| {
                        de::Error::custom(format!(
                            "Failed to deserialize AccountToCustody body: {}",
                            e
                        ))
                    })?,
                    "CustodyToAccount" => serde_json::from_value(serde_json::Value::Object(
                        body_fields,
                    ))
                    .map_err(|e| {
                        de::Error::custom(format!(
                            "Failed to deserialize CustodyToAccount body: {}",
                            e
                        ))
                    })?,
                    _ => {
                        return Err(de::Error::custom(format!(
                            "Unknown message type: {}",
                            msg_type
                        )))
                    }
                };

                Ok(FixRequest {
                    session_id: SessionId::from("S-1"),
                    standard_header,
                    chain_id,
                    address,
                    body,
                    standard_trailer,
                })
            }
        }

        deserializer.deserialize_struct(
            "FixRequest",
            &[
                "standard_header",
                "chain_id",
                "address",
                "body",
                "standard_trailer",
            ],
            FixRequestVisitor,
        )
    }
}

impl AxumServerRequest for FixRequest {
    fn deserialize_from_fix(
        message: FixMessage,
        this_session_id: &SessionId,
    ) -> Result<Self, eyre::Error> {
        let mut request: FixRequest = serde_json::from_str(&message.to_string())
            .map_err(|e| eyre!("Failed to deserialize FixMessage into FixRequest: {}", e))?;

        // Set the session_id
        request.session_id = this_session_id.clone();
        tracing::debug!(
            session_id = %request.session_id,
            msg_type = %request.standard_header.msg_type,
            "FIX server request received",
        );
        Ok(request)
    }
}

impl FixRequest {
    pub fn verify_signature(&self) -> Result<()> {
        // Skip quote messages
        let msg_type = &self.standard_header.msg_type;
        if msg_type.contains("Quote") {
            return Ok(());
        }

        // 1) Trailer
        let trailer = self
            .standard_trailer
            .as_ref()
            .ok_or_else(|| eyre!("Missing trailer"))?;

        let pub_key_hex = trailer
            .public_key
            .get(0)
            .ok_or_else(|| eyre!("Missing public key"))?
            .trim_start_matches("0x");

        let sig_hex = trailer
            .signature
            .get(0)
            .ok_or_else(|| eyre!("Missing signature"))?
            .trim_start_matches("0x");

        // 2) Parse uncompressed SEC1 pubkey (65B, 0x04 + X + Y)
        let pub_key_bytes = hex::decode(pub_key_hex)?;
        if pub_key_bytes.len() != 65 || pub_key_bytes[0] != 0x04 {
            return Err(eyre!("Invalid uncompressed SEC1 public key format"));
        }

        // 3) Derived address must match provided address
        let mut hasher = Keccak256::new();
        hasher.update(&pub_key_bytes[1..]); // drop 0x04 prefix
        let pubhash: [u8; 32] = hasher.finalize().into();
        let derived_addr = &pubhash[12..]; // last 20 bytes

        let provided_addr: [u8; 20] = self.address.into();

        println!(
            "[BE] derived_addr: 0x{}, provided_addr: 0x{}",
            hex::encode(derived_addr),
            hex::encode(provided_addr)
        );

        if derived_addr != provided_addr {
            return Err(eyre!(
                "Invalid address (pubkey does not match address field)"
            ));
        }

        // 4) Build the exact FE signable string
        let id = match &self.body {
            RequestBody::NewIndexOrderBody {
                client_order_id, ..
            }
            | RequestBody::CancelIndexOrderBody {
                client_order_id, ..
            } => client_order_id,
            RequestBody::NewQuoteRequestBody {
                client_quote_id, ..
            }
            | RequestBody::CancelQuoteRequestBody {
                client_quote_id, ..
            } => client_quote_id,
            _ => return Err(eyre!("Unsupported msg_type")),
        };
        let signable = format!(r#"{{"msg_type":"{}","id":"{}"}}"#, msg_type, id);

        // 5) Compute digest (EIP-191)
        let digest: [u8; 32] = hash_message(&signable).into();

        // 6) Parse signature: accept 64B (r||s) or 65B (r||s||v)
        let mut sig_bytes = hex::decode(sig_hex)?;
        if sig_bytes.len() == 65 {
            sig_bytes.truncate(64); // drop v
        }
        if sig_bytes.len() != 64 {
            return Err(eyre!(
                "Signature must be 64 bytes (r||s) or 65 bytes (r||s||v)"
            ));
        }

        let mut signature = Signature::try_from(sig_bytes.as_slice())
            .map_err(|e| eyre!("Invalid signature: {}", e))?;

        let signature = signature.normalize_s().unwrap_or(signature);
        // 7) Verify
        let verifying_key = VerifyingKey::from_sec1_bytes(&pub_key_bytes)
            .map_err(|e| eyre!("Invalid public key: {}", e))?;
        verifying_key
            .verify_prehash(digest.as_ref(), &signature)
            .map_err(|_| eyre!("Signature verification failed"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers_core::utils::keccak256;
    use serde_json::json;

    #[test]
    fn test_signature_verification_with_static_data() {
        use serde_json::json;

        // values captured from FE logs
        let pubkey_hex = "0x04f5507523649a16a3db67d295f3a5145a917c6a007193e3710ef04783332bdaa05ab2ad9d312f32e92c1ac96129535e02ca65ead06e1e0d638beda24660354401";
        let signature_hex = "0xd5572e67c0b39f6c8353a4cf2f690aa157715a5d23a4f171bd1f22033b810778792660e441f10eff811b8020caf3faad3ee176e5fa9e820fe5539933a9c0e40a";

        let payload = json!({
          "standard_header": {
            "msg_type": "NewIndexOrder",
            "sender_comp_id": "FE",
            "target_comp_id": "SERVER",
            "seq_num": 1,
            "timestamp": "2025-09-22T16:24:53.613Z"
          },
          "chain_id": 8453,
          "address": "0x73E18f4d1bEaD416624ebF3DB192f14641721048",
          "client_order_id": "VJV-TLL-BEQ-8403",
          "symbol": "SY100",
          "side": "1",
          "amount": "0",
          "standard_trailer": {
            "public_key": [pubkey_hex],
            "signature": [signature_hex]
          }
        });

        // Deserialize into your FixRequest type
        let mut fix: FixRequest = serde_json::from_value(payload).unwrap();
        fix.session_id = SessionId::from("S-1");

        // Run verification
        let result = fix.verify_signature();
        assert!(
            result.is_ok(),
            "Signature verification failed: {:?}",
            result.err()
        );
    }
    #[test]
    fn test_quote_request_without_signature() {
        let fix_json = json!({
          "standard_header": {
            "msg_type": "NewQuoteRequest",
            "sender_comp_id": "CLIENT",
            "target_comp_id": "SERVER",
            "seq_num": 1,
            "timestamp": "2025-07-30T10:13:59.648Z"
          },
          "chain_id": 1,
          "address": "0xc7dd6ddef2b3038286616b8b3a01c6bdc3b4726a",
          "client_quote_id": "Q-1753870439648",
          "symbol": "SY100",
          "side": "1",
          "amount": "1000"
        });

        let mut fix: FixRequest = serde_json::from_value(fix_json).unwrap();
        fix.session_id = SessionId::from("S-1");

        // Should pass, since it's a quote request and signature is not required
        assert!(
            fix.verify_signature().is_ok(),
            "Quote request without signature should pass verification"
        );
    }

    #[test]
    fn test_with_rate_limit_plugin_implementation() {
        use axum_fix_server::plugins::rate_limit_plugin::{
            MessageType, RateLimitKey, WithRateLimitPlugin,
        };
        use symm_core::core::test_util::get_mock_address_1;

        let user_id = (1, get_mock_address_1());
        let session_id = SessionId::from("test_session");

        // Test different message types and their weights/classifications
        let test_cases = vec![
            ("NewIndexOrder", 10, MessageType::Order),
            ("CancelIndexOrder", 5, MessageType::Order),
            ("NewQuoteRequest", 5, MessageType::Quote),
            ("CancelQuoteRequest", 3, MessageType::Quote),
            ("AccountToCustody", 1, MessageType::Administrative),
            ("CustodyToAccount", 1, MessageType::Administrative),
            ("Heartbeat", 1, MessageType::Administrative),
            ("UnknownMessage", 1, MessageType::Administrative), // Default case
        ];

        for (msg_type, expected_weight, expected_type) in test_cases {
            let request = FixRequest {
                session_id: session_id.clone(),
                standard_header: FixHeader {
                    msg_type: msg_type.to_string(),
                    sender_comp_id: "CLIENT".to_string(),
                    target_comp_id: "SERVER".to_string(),
                    seq_num: 1,
                    timestamp: chrono::Utc::now(),
                },
                chain_id: user_id.0,
                address: user_id.1,
                body: RequestBody::NewIndexOrderBody {
                    client_order_id: "O123".to_string(),
                    symbol: "BTC".to_string(),
                    side: "1".to_string(),
                    amount: "100".to_string(),
                },
                standard_trailer: Some(FixTrailer::new()),
            };

            // Test trait implementation
            assert_eq!(
                axum_fix_server::plugins::rate_limit_plugin::WithRateLimitPlugin::get_user_id(
                    &request
                ),
                user_id
            );
            assert_eq!(
                request.get_rate_limit_key(),
                RateLimitKey::User(user_id.0, user_id.1)
            );
            assert_eq!(
                request.get_message_weight(),
                expected_weight,
                "Message type {} should have weight {}",
                msg_type,
                expected_weight
            );
            assert_eq!(
                request.get_message_type(),
                expected_type,
                "Message type {} should be classified as {:?}",
                msg_type,
                expected_type
            );
        }
    }

    #[test]
    fn test_rate_limit_key_consistency() {
        use axum_fix_server::plugins::rate_limit_plugin::{RateLimitKey, WithRateLimitPlugin};
        use symm_core::core::test_util::{get_mock_address_1, get_mock_address_2};

        let user_id_1 = (1, get_mock_address_1());
        let user_id_2 = (2, get_mock_address_2());
        let session_id = SessionId::from("key_test_session");

        let request_1a = create_test_request(&user_id_1, &session_id, "NewIndexOrder");
        let request_1b = create_test_request(&user_id_1, &session_id, "CancelIndexOrder");
        let request_2 = create_test_request(&user_id_2, &session_id, "NewIndexOrder");

        // Same user should have same rate limit key regardless of message type
        assert_eq!(
            request_1a.get_rate_limit_key(),
            request_1b.get_rate_limit_key()
        );

        // Different users should have different rate limit keys
        assert_ne!(
            request_1a.get_rate_limit_key(),
            request_2.get_rate_limit_key()
        );

        // Verify key structure
        assert_eq!(
            request_1a.get_rate_limit_key(),
            RateLimitKey::User(1, get_mock_address_1())
        );
        assert_eq!(
            request_2.get_rate_limit_key(),
            RateLimitKey::User(2, get_mock_address_2())
        );
    }

    // Helper function for creating test requests
    fn create_test_request(
        user_id: &(u32, Address),
        session_id: &SessionId,
        msg_type: &str,
    ) -> FixRequest {
        FixRequest {
            session_id: session_id.clone(),
            standard_header: FixHeader {
                msg_type: msg_type.to_string(),
                sender_comp_id: "CLIENT".to_string(),
                target_comp_id: "SERVER".to_string(),
                seq_num: 1,
                timestamp: chrono::Utc::now(),
            },
            chain_id: user_id.0,
            address: user_id.1,
            body: RequestBody::NewIndexOrderBody {
                client_order_id: "O123".to_string(),
                symbol: "BTC".to_string(),
                side: "1".to_string(),
                amount: "100".to_string(),
            },
            standard_trailer: Some(FixTrailer::new()),
        }
    }
}
