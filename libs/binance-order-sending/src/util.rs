use eyre::{eyre, Result};
use itertools::Itertools;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use crate::{credentials::Credentials, signing::sign};

pub fn build_request_maybe_signed(
    request_id: &str,
    method: &str,
    mut params: BTreeMap<String, String>,
    timestamp: u128,
    recv_window: Option<u32>,
    credentials: Option<Credentials>,
) -> Result<String> {
    params.insert("timestamp".to_owned(), timestamp.to_string());
    if let Some(recv_window) = recv_window {
        params.insert("recvWindow".to_string(), recv_window.to_string());
    }

    if let Some(credentials) = credentials {
        params.insert("apiKey".to_string(), credentials.api_key);

        let payload = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect_vec()
            .join("&");

        let signature_base64 = sign(payload.as_str(), &credentials.signature)
            .map_err(|err| eyre!("Failed to sign request {}", err))?;

        params.insert("signature".to_string(), signature_base64);
    }

    let mut json_params = Map::new();
    for (k, v) in params {
        json_params.insert(k, Value::String(v));
    }

    let request = json!({
        "id": request_id,
        "method": method,
        "params": json_params
    });

    Ok(request.to_string())
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use serde_json::{json, Value};

    use crate::{credentials::Credentials, util::build_request_maybe_signed};

    #[test]
    fn test_format_request() {
        // example data from: https://developers.binance.com/docs/binance-spot-api-docs/websocket-api/request-security
        let credentials = Credentials::from_hmac(
            "vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A",
            "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j",
        );
        let timestamp = 1645423376532;
        let request = build_request_maybe_signed(
            "4885f793-e5ad-4c3b-8f6c-55d891472b71",
            "order.place",
            BTreeMap::from_iter(
                [
                    ("symbol", "BTCUSDT"),
                    ("side", "SELL"),
                    ("type", "LIMIT"),
                    ("timeInForce", "GTC"),
                    ("quantity", "0.01000000"),
                    ("price", "52000.00"),
                    ("newOrderRespType", "ACK"),
                ]
                .into_iter()
                .map(|(a, b)| (a.to_owned(), b.to_owned())),
            ),
            timestamp,
            Some(100),
            Some(credentials),
        )
        .expect("Failed to format request");

        println!("Formatted Request: {}", request);

        let result: Value = serde_json::from_str(&request).expect("Failed to parse json result");
        let expected = json!({
          "id": "4885f793-e5ad-4c3b-8f6c-55d891472b71",
          "method": "order.place",
          "params": {
            "symbol":           "BTCUSDT",
            "side":             "SELL",
            "type":             "LIMIT",
            "timeInForce":      "GTC",
            "quantity":         "0.01000000",
            "price":            "52000.00",
            "newOrderRespType": "ACK",
            "recvWindow":       "100",
            "timestamp":        "1645423376532",
            "apiKey":           "vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A",
            "signature":        "cc15477742bd704c29492d96c7ead9414dfd8e0ec4a00f947bb5bb454ddbd08a"
          }
        });
        assert_eq!(result, expected);
    }
}
