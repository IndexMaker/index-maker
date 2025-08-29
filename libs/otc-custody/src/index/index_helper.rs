use alloy::{
    primitives::{keccak256, Address, Bytes, U256},
    sol_types::SolValue,
};
use struple::Struple;

pub(crate) const OTC_INDEX_CONNECTOR_TYPE: &str = "OTCIndex";
pub(crate) const OTC_INDEX_CONNECTOR_NAME: &str = "OTCIndexConnector";
pub(crate) const OTC_INDEX_DEPLOY_CUSTODY_STATE: u8 = 0;
pub(crate) const OTC_INDEX_CURATOR_WEIGHTS_CUSTODY_STATE: u8 = 0;
pub(crate) const OTC_INDEX_SOLVER_WEIGHTS_SET_STATE: u8 = 0;
pub(crate) const OTC_INDEX_MINT_CURATOR_STATE: u8 = 0;
pub(crate) const OTC_TRADE_ROUTE_CUSTODY_STATE: u8 = 0;
pub(crate) const OTC_WITHDRAW_ROUTE_CUSTODY_STATE: u8 = 0;

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

pub fn encode_mint_call(to: Address, amount: U256, seq: U256) -> Bytes {
    let sel = keccak256(b"mint(address,uint256,uint256)");
    let mut out = Vec::with_capacity(4 + 32 * 3);
    out.extend_from_slice(&sel[..4]);
    let enc = SolValue::abi_encode_params(&(to, amount, seq));
    out.extend_from_slice(&enc);
    out.into()
}
