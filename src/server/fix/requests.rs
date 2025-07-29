use std::fmt;

use axum_fix_server::{
    messages::{FixMessage, ServerRequest as AxumServerRequest, SessionId},
    plugins::{seq_num_plugin::WithSeqNumPlugin, user_plugin::WithUserPlugin},
};
use ethers_core::utils::keccak256;
use eyre::{eyre, Result};
use hex;
use k256::ecdsa::{VerifyingKey};
use k256::ecdsa::{signature::Verifier, Signature};
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::json;
use symm_core::core::bits::Address;

use crate::server::fix::messages::*;

#[derive(Serialize, Debug)]
pub struct FixRequest {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    pub chain_id: u32,
    pub address: Address,
    #[serde(flatten)]
    pub body: RequestBody,
    pub standard_trailer: FixTrailer,
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
                let standard_trailer =
                    standard_trailer.ok_or_else(|| de::Error::missing_field("standard_trailer"))?;

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
        tracing::info!(
            session_id = %request.session_id,
            msg_type = %request.standard_header.msg_type,
            "FIX server request received",
        );
        Ok(request)
    }
}

impl FixRequest {
    pub fn verify_signature(&self) -> Result<()> {
        let msg_type = self.standard_header.msg_type.to_ascii_lowercase();

        // Skip signature verification checking for quote-related messages
        if msg_type.contains("quote") {
            return Ok(());
        }
        let payload_json = json!({
            "standard_header": self.standard_header,
            "chain_id": self.chain_id,
            "address": self.address,
            "body": self.body
        });

        let payload_bytes = serde_json::to_vec(&payload_json)
            .map_err(|e| eyre!("Payload serialization failed: {}", e))?;

        let hash = keccak256(payload_bytes);

        let sig_hex = self
            .standard_trailer
            .signature
            .get(0)
            .ok_or_else(|| eyre!("Missing signature"))?;

        let pub_key_hex = self
            .standard_trailer
            .public_key
            .get(0)
            .ok_or_else(|| eyre!("Missing public key"))?;

        let sig_bytes = hex::decode(sig_hex).map_err(|e| eyre!("Bad signature hex: {}", e))?;
        let pub_key_bytes = hex::decode(pub_key_hex).map_err(|e| eyre!("Bad pubkey hex: {}", e))?;

        let verifying_key = VerifyingKey::from_sec1_bytes(&pub_key_bytes)
            .map_err(|e| eyre!("Invalid pubkey bytes: {}", e))?;

        let signature =
            Signature::from_der(&sig_bytes).map_err(|e| eyre!("Signature parse failed: {}", e))?;

        verifying_key
            .verify(&hash, &signature)
            .map_err(|_| eyre!("Signature verification failed"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers_core::utils::keccak256;
    use k256::ecdsa::{signature::Signer, Signature, SigningKey};
    use rand_core::OsRng;

    #[test]
    fn test_signature_verification() {
        let signing_key = SigningKey::random(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let encoded_point = verifying_key.to_encoded_point(false);
        let pubkey_bytes = encoded_point.as_bytes();
        let pubkey_hex = hex::encode(pubkey_bytes);

        let dummy_address = Address::from_slice(&[0u8; 20]); // placeholder, not validated

        let header = FixHeader {
            msg_type: "NewIndexOrder".to_string(),
            sender_comp_id: "client".to_string(),
            target_comp_id: "server".to_string(),
            seq_num: 1,
            timestamp: chrono::Utc::now(),
        };

        let body = RequestBody::NewIndexOrderBody {
            client_order_id: "ORD001".to_string(),
            symbol: "ETHUSD".to_string(),
            side: "buy".to_string(),
            amount: "10".to_string(),
        };

        let chain_id = 1;

        let payload = serde_json::json!({
            "standard_header": header,
            "chain_id": chain_id,
            "address": dummy_address,
            "body": body,
        });

        let hash = keccak256(serde_json::to_vec(&payload).unwrap());
        let sig: Signature = signing_key.sign(&hash);
        let sig_hex = hex::encode(sig.to_der().as_bytes());

        let req = FixRequest {
            session_id: SessionId::from("test-session"),
            standard_header: header,
            chain_id,
            address: dummy_address,
            body,
            standard_trailer: FixTrailer {
                public_key: vec![pubkey_hex],
                signature: vec![sig_hex],
            },
        };

        assert!(
            req.verify_signature().is_ok(),
            "Signature should verify when public key is included"
        );
    }
}
