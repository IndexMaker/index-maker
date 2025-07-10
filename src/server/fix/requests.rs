use std::fmt;

use axum_fix_server::{
    messages::{FixMessage, ServerRequest as AxumServerRequest, SessionId},
    plugins::{seq_num_plugin::WithSeqNumPlugin, user_plugin::WithUserPlugin},
};
use eyre::{eyre, Result};
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize,
};
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
    pub body: Body,
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

                        Body::NewIndexOrderBody {
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

                        Body::CancelIndexOrderBody {
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

                        Body::NewQuoteRequestBody {
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

                        Body::CancelQuoteRequestBody {
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
            "FIX server request received from {}: {}",
            request.session_id,
            request.standard_header.msg_type
        );
        Ok(request)
    }
}
