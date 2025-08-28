use alloy::{
    primitives::{keccak256, Address, Bytes, B256, U256},
    sol_types::SolValue,
};
use struple::Struple;

#[derive(Struple, Debug, PartialEq, Clone)]
pub struct IndexDeployData {
    pub name: String,
    pub symbol: String,
    pub collateral_token: Address,
    pub collateral_token_precision: U256,
    pub management_fee: U256,
    pub performance_fee: U256,
    pub max_mint_per_block: U256,
    pub max_redeem_per_block: U256,
    pub vote_threshold: U256,
    pub vote_period: U256,
    pub initial_price: U256,
}

impl IndexDeployData {
    pub fn to_encoded_params(self) -> Bytes {
        SolValue::abi_encode_params(&self.into_tuple()).into()
    }
}

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

pub fn encode_mint_call(to: Address, amount: U256, seq: U256) -> Bytes {
    let sel = keccak256(b"mint(address,uint256,uint256)");
    let mut out = Vec::with_capacity(4 + 32 * 3);
    out.extend_from_slice(&sel[..4]);
    let enc = SolValue::abi_encode_params(&(to, amount, seq));
    out.extend_from_slice(&enc);
    out.into()
}
