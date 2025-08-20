use crate::contracts::{CAKey, Signature, VerificationData};
use alloy_primitives::{fixed_bytes, U256};

pub fn build_verification_data() -> VerificationData {
    let verification_data = VerificationData {
        id: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
        state: 0u8,
        timestamp: U256::from(0),
        pubKey: CAKey {
            parity: 0u8,
            x: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
        },
        sig: Signature {
            e: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
            s: fixed_bytes!("0x0000000000000000000000000000000000000000000000000000000000000000"),
        },
        merkleProof: vec![fixed_bytes!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        )],
    };
    verification_data
}
