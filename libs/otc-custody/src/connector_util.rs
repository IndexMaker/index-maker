use alloy::{
    primitives::{keccak256, Address, Bytes, B256, U256},
    sol_types::SolValue,
};

pub fn deploy_connector_message(
    ts: U256,
    id: B256,
    connector_type: &str,
    factory: Address,
    data: &Bytes,
) -> Bytes {
    let data_hash = B256::from(keccak256(data));
    SolValue::abi_encode_packed(&(
        ts,
        "deployConnector".to_string(),
        id,
        connector_type.to_string(),
        factory,
        data_hash,
    ))
    .into()
}

pub fn call_connector_message(
    ts: U256,
    id: B256,
    connector_type: &str,
    connector_addr: Address,
    fixed: &Bytes,
    tail: &Bytes,
) -> Bytes {
    let fixed_hash = B256::from(keccak256(fixed));
    let tail_hash = B256::from(keccak256(tail));
    SolValue::abi_encode_packed(&(
        ts,
        "callConnector".to_string(),
        id,
        connector_type.to_string(),
        connector_addr,
        fixed_hash,
        tail_hash,
    ))
    .into()
}

pub fn custody_to_address_message(
    ts: U256,
    id: B256,
    token: Address,
    destination: Address,
    amount: U256,
) -> Bytes {
    SolValue::abi_encode_packed(&(
        ts,
        "custodyToAddress".to_string(),
        id,
        token,
        destination,
        amount,
    ))
    .into()
}

pub fn encode_with_selector(sig: &str, ts: U256, weights: &Bytes, price: U256) -> Bytes {
    let sel = keccak256(sig.as_bytes());
    let mut out = Vec::with_capacity(4 + 128);
    out.extend_from_slice(&sel[..4]);

    let enc = SolValue::abi_encode_params(&(ts, weights.clone(), price));
    out.extend_from_slice(&enc);
    out.into()
}
