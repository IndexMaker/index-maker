use alloy::{
    hex,
    primitives::{keccak256, Address, Bytes, FixedBytes, B256},
    sol_types::SolValue,
};
use eyre::eyre;
use k256::ecdsa::SigningKey;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::elliptic_curve::{bigint::U256 as ECUint, ops::Reduce};
use k256::ProjectivePoint;
use k256::{elliptic_curve::PrimeField, FieldBytes, Scalar};

// ---------- helpers: key → (parity, x) for Schnorr pubkey ----------

pub fn signing_key_bytes_from_hex(sk_hex: String) -> eyre::Result<[u8; 32]> {
    let sk_vec = hex::decode(sk_hex.trim_start_matches("0x"))?;
    let sk_bytes: [u8; 32] = sk_vec
        .try_into()
        .map_err(|err| eyre!("Failed to parse key: {:?}", err))?;
    Ok(sk_bytes)
}

pub fn pubkey_parity_27_28(sk_bytes: &[u8; 32]) -> eyre::Result<(u8, B256)> {
    let sk = SigningKey::from_slice(sk_bytes)?;
    let vk = sk.verifying_key();

    let ep = vk.to_encoded_point(true); // 33 bytes: 0x02/0x03 || X
    let tag = ep.as_bytes()[0];
    let parity = if tag == 0x02 { 27u8 } else { 28u8 }; // this is required from our Solidity contracts
    let x_bytes = &ep.as_bytes()[1..33];
    Ok((parity, B256::from_slice(x_bytes)))
}

pub fn schnorr_sign_per_contract(
    sk_bytes: &[u8; 32],
    parity: u8, // 27/28
    px: B256,   // pubkey X
    message_data: &[u8],
) -> eyre::Result<(B256, B256)> {
    let x = Scalar::from_repr(FieldBytes::clone_from_slice(sk_bytes))
        .into_option()
        .ok_or_else(|| eyre::eyre!("sk out of range"))?;

    // message = keccak256(messageData)
    let message = B256::from(keccak256(message_data));

    // deterministic k = reduce(keccak256(sk || message))
    let mut kin = Vec::with_capacity(64);
    kin.extend_from_slice(sk_bytes);
    kin.extend_from_slice(message.as_slice());
    let k = Scalar::reduce(ECUint::from_be_slice(keccak256(&kin).as_slice()));

    // R = k·G, ethereum address of R (keccak of uncompressed XY, take last 20 bytes)
    let r_aff = (ProjectivePoint::GENERATOR * k).to_affine();
    let r_uncompressed = r_aff.to_encoded_point(false); // 0x04 || X || Y (65 bytes)
    let r_hash = keccak256(&r_uncompressed.as_bytes()[1..]); // hash 64-byte X||Y
    let r_addr = Address::from_slice(&r_hash[12..]); // last 20 bytes

    // e = keccak256(abi.encodePacked(R_address, parity, px, message))
    let parity_b1: FixedBytes<1> = [parity].into();
    let e_bytes: Bytes = SolValue::abi_encode_packed(&(r_addr, parity_b1, px, message)).into();
    let e_b256 = B256::from(keccak256(e_bytes.as_ref()));
    let e = Scalar::reduce(ECUint::from_be_slice(e_b256.as_slice()));

    // s = k + e*x  (mod Q)
    let s = k + e * x;

    Ok((e_b256, B256::from_slice(s.to_bytes().as_slice())))
}
