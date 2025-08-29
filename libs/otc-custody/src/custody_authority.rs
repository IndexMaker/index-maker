use std::sync::Arc;

use alloy::primitives::{Bytes, B256, U256};
use k256::elliptic_curve::zeroize::Zeroize;

use crate::{
    contracts::{SchnorrCAKey, SchnorrSignature, VerificationData},
    custody_helper::Party,
    schnorr::{pubkey_parity_27_28, schnorr_sign_per_contract, signing_key_bytes_from_hex},
};

pub trait CustodyAuthorityMethods {
    fn get_party(&self) -> eyre::Result<Party>;
    fn get_verification_data(
        &self,
        custody_id: B256,
        custody_state: u8,
        timestamp: U256,
        proof: &Vec<B256>,
        message: &Bytes,
    ) -> eyre::Result<VerificationData>;
}

pub struct CustodyAuthorityImpl<GetSigningKeyFn>
where
    GetSigningKeyFn: Fn() -> String + Send + Sync + 'static,
{
    get_signing_key: GetSigningKeyFn,
}

impl<GetSigningKeyFn> CustodyAuthorityImpl<GetSigningKeyFn>
where
    GetSigningKeyFn: Fn() -> String + Send + Sync + 'static,
{
    pub fn new(get_signing_key: GetSigningKeyFn) -> Self {
        Self { get_signing_key }
    }
}

impl<GetSigningKeyFn> CustodyAuthorityMethods for CustodyAuthorityImpl<GetSigningKeyFn>
where
    GetSigningKeyFn: Fn() -> String + Send + Sync + 'static,
{
    fn get_party(&self) -> eyre::Result<Party> {
        let signing_key_hex = (self.get_signing_key)();
        let mut signing_key_bytes = signing_key_bytes_from_hex(signing_key_hex)?;
        let (parity, x) = pubkey_parity_27_28(&signing_key_bytes)?;
        signing_key_bytes.zeroize();
        Ok(Party { parity, x })
    }

    fn get_verification_data(
        &self,
        custody_id: B256,
        custody_state: u8,
        timestamp: U256,
        proof: &Vec<B256>,
        message: &Bytes,
    ) -> eyre::Result<VerificationData> {
        let signing_key_hex = (self.get_signing_key)();
        let mut signing_key_bytes = signing_key_bytes_from_hex(signing_key_hex)?;
        let (parity, x) = pubkey_parity_27_28(&signing_key_bytes)?;
        let (e, s) = schnorr_sign_per_contract(&signing_key_bytes, parity, x, message)?;
        signing_key_bytes.zeroize();
        Ok(VerificationData {
            id: custody_id,
            state: custody_state,
            timestamp,
            pubKey: SchnorrCAKey { parity, x },
            sig: SchnorrSignature { e, s },
            merkleProof: proof.clone(),
        })
    }
}

#[derive(Clone)]
pub struct CustodyAuthority(Arc<dyn CustodyAuthorityMethods + Send + Sync + 'static>);

impl CustodyAuthority {
    pub fn new(get_signing_key: impl Fn() -> String + Send + Sync + 'static) -> CustodyAuthority {
        CustodyAuthority(Arc::new(CustodyAuthorityImpl::new(get_signing_key)))
    }

    pub fn get_party(&self) -> eyre::Result<Party> {
        self.0.get_party()
    }

    pub fn get_verification_data(
        &self,
        custody_id: B256,
        custody_state: u8,
        timestamp: U256,
        proof: &Vec<B256>,
        message: &Bytes,
    ) -> eyre::Result<VerificationData> {
        self.0
            .get_verification_data(custody_id, custody_state, timestamp, proof, message)
    }
}
