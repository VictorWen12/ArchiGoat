//! Keyed proofs bind private handoffs and native runners to one ArchiGoat identity.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
// SHA-256 supplies the digest used by every keyed proof in this module.
use sha2::Sha256;

// This digest type authenticates device-owned Work commands and receipts.
type HmacSha256 = Hmac<Sha256>;

/// ChallengeResponse lets a live owner prove its private installation identity.
#[derive(Deserialize, Serialize)]
pub(crate) struct ChallengeResponse {
    pub(crate) server_nonce: String,
    pub(crate) proof: String,
}

// Nonces use operating-system entropy so private claims cannot be predicted.
pub(crate) fn nonce() -> Result<String, String> {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(|error| format!("Could not generate nonce: {error}"))?;
    Ok(hex(&random))
}

// Fixed-shape nonces prevent path and delimiter confusion at process boundaries.
pub(crate) fn valid_nonce(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// Host proofs admit only the exact Work bytes signed by this ArchiGoat instance.
pub(crate) fn host_proof(secret: &str, payload: &[u8]) -> Result<String, String> {
    sign_bytes(secret, b"terminal-work:", payload)
}

// Constant-time verification rejects forged Work requests from sibling processes.
pub(crate) fn verify_host(secret: &str, payload: &[u8], proof: &str) -> bool {
    verify_bytes(secret, b"terminal-work:", payload, proof)
}

// Server proof binds both fresh nonces before a secondary process may claim the loopback port.
pub(crate) fn server_proof(
    secret: &str,
    client_nonce: &str,
    server_nonce: &str,
) -> Result<String, String> {
    sign_bytes(
        secret,
        b"loopback-server:",
        format!("{client_nonce}:{server_nonce}").as_bytes(),
    )
}

// The secondary accepts only a challenge signed by this installation's durable secret.
pub(crate) fn verify_server(
    secret: &str,
    client_nonce: &str,
    server_nonce: &str,
    proof: &str,
) -> bool {
    verify_bytes(
        secret,
        b"loopback-server:",
        format!("{client_nonce}:{server_nonce}").as_bytes(),
        proof,
    )
}

// Exit proof binds a newer instance's protocol claim to the one authenticated nonce exchange.
pub(crate) fn exit_proof(
    secret: &str,
    client_nonce: &str,
    server_nonce: &str,
    protocol: u16,
) -> Result<String, String> {
    sign_bytes(
        secret,
        b"loopback-exit:",
        format!("{client_nonce}:{server_nonce}:{protocol}").as_bytes(),
    )
}

// The owner yields the port only to a proven same-installation instance with a newer protocol.
pub(crate) fn verify_exit(
    secret: &str,
    client_nonce: &str,
    server_nonce: &str,
    protocol: u16,
    proof: &str,
) -> bool {
    verify_bytes(
        secret,
        b"loopback-exit:",
        format!("{client_nonce}:{server_nonce}:{protocol}").as_bytes(),
        proof,
    )
}

// Byte signing authenticates serialized handoff payloads without lossy text conversion.
fn sign_bytes(secret: &str, domain: &[u8], payload: &[u8]) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| "Instance key is invalid".to_owned())?;
    mac.update(domain);
    mac.update(payload);
    Ok(hex(&mac.finalize().into_bytes()))
}

// Byte verification uses constant-time HMAC comparison for runner handoff proofs.
fn verify_bytes(secret: &str, domain: &[u8], payload: &[u8], proof: &str) -> bool {
    let Some(bytes) = decode_hex(proof) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(domain);
    mac.update(payload);
    mac.verify_slice(&bytes).is_ok()
}

// Lowercase hex preserves proof bytes across text-only process handoffs.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// Exact byte decoding rejects malformed proof text before verification.
fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}
