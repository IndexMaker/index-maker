use alloy_primitives::{fixed_bytes, FixedBytes, U256};
use ca_helper::contracts::{SchnorrCAKey, SchnorrSignature, VerificationData};

pub fn build_id(id: U256) -> FixedBytes<32> {
    let _ = id;
    fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000")
}

pub fn build_verification_data() -> VerificationData {
    let verification_data = VerificationData {
        id: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
        state: 0u8,
        timestamp: U256::from(0),
        pubKey: SchnorrCAKey {
            parity: 0u8,
            x: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
        },
        sig: SchnorrSignature {
            e: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
            s: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
        },
        merkleProof: vec![fixed_bytes!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        )],
    };
    verification_data
}
