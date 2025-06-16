use base64::{engine::general_purpose, Engine as _};
use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::SigningKey;
use ed25519_dalek::{Signature as Ed25519Signature, Signer};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::error::Error;

use crate::credentials::Signature;

pub fn sign(payload: &str, signature: &Signature) -> Result<String, Box<dyn Error>> {
    match signature {
        Signature::Hmac(signature) => sign_hmac(payload, &signature.api_secret),
        Signature::Ed25519(signature) => sign_ed25519(payload, &signature.key),
    }
}

fn sign_hmac(payload: &str, key: &str) -> Result<String, Box<dyn Error>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.to_string().as_bytes())?;

    mac.update(payload.to_string().as_bytes());
    let result = mac.finalize();
    Ok(format!("{:x}", result.into_bytes()))
}

fn sign_ed25519(payload: &str, key: &str) -> Result<String, Box<dyn Error>> {
    let private_key = SigningKey::from_pkcs8_pem(key)?;

    let signature: Ed25519Signature = private_key.sign(payload.as_bytes());
    Ok(general_purpose::STANDARD.encode(signature.to_bytes()))
}
