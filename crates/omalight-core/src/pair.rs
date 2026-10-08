//! Numeric comparison for the first connection.
//!
//! Both sides run X25519. The code is the first 4 bytes of
//! SHA-256("omalight-pair-v1" || client_pub || server_pub || shared), taken mod 1_000_000.
//! A relay that substitutes a key gets a different code on each side, so the user will not confirm.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey};

pub const LABEL: &[u8] = b"omalight-pair-v1";

pub fn pairing_code(client_pub: &[u8], server_pub: &[u8], shared: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(LABEL);
    hasher.update(client_pub);
    hasher.update(server_pub);
    hasher.update(shared);
    let digest = hasher.finalize();
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{n:06}")
}

pub fn encode_key(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn decode_key(text: &str) -> Result<[u8; 32], String> {
    let bytes = STANDARD
        .decode(text.trim())
        .map_err(|e| format!("pairing key: {e}"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| "pairing key is not 32 bytes".to_string())
}

pub fn client_keypair() -> (EphemeralSecret, PublicKey) {
    let secret = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let public = PublicKey::from(&secret);
    (secret, public)
}

pub fn server_keypair() -> (EphemeralSecret, PublicKey) {
    client_keypair()
}

pub fn shared_code(
    secret: EphemeralSecret,
    ours: &PublicKey,
    theirs: &PublicKey,
    client_first: bool,
) -> String {
    let shared = secret.diffie_hellman(theirs);
    let (client_pub, server_pub) = if client_first {
        (ours.as_bytes().as_slice(), theirs.as_bytes().as_slice())
    } else {
        (theirs.as_bytes().as_slice(), ours.as_bytes().as_slice())
    };
    pairing_code(client_pub, server_pub, shared.as_bytes())
}

/// `ssh` stdout from `omalight-engine pair-ssh`. Anything else is a failure.
pub fn parse_ssh_credential(stdout: &str) -> Result<String, String> {
    for line in stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix("credential ") {
            let token = rest.trim();
            if token.len() >= 16 && token.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(token.to_string());
            }
        }
    }
    Err("ssh did not return a credential".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_derive_the_same_code() {
        let (client_secret, client_pub) = client_keypair();
        let (server_secret, server_pub) = server_keypair();
        let client_code = shared_code(client_secret, &client_pub, &server_pub, true);
        let server_code = shared_code(server_secret, &server_pub, &client_pub, false);
        assert_eq!(client_code, server_code);
        assert_eq!(client_code.len(), 6);
        assert!(client_code.chars().all(|c| c.is_ascii_digit()));
        let swapped = pairing_code(server_pub.as_bytes(), client_pub.as_bytes(), &[0u8; 32]);
        assert_ne!(client_code, swapped);
    }

    #[test]
    fn pairing_code_is_stable() {
        let code = pairing_code(
            b"client-key-material!!",
            b"server-key-material!!",
            b"shared-secret-material!",
        );
        assert_eq!(
            code,
            pairing_code(
                b"client-key-material!!",
                b"server-key-material!!",
                b"shared-secret-material!"
            )
        );
        assert_ne!(
            code,
            pairing_code(
                b"client-key-material!?",
                b"server-key-material!!",
                b"shared-secret-material!"
            )
        );
        assert_eq!(code.len(), 6);
    }

    #[test]
    fn ssh_credential_line() {
        let line = "listening\ncredential 0123456789abcdef0123456789abcdef\n";
        assert_eq!(
            parse_ssh_credential(line).unwrap(),
            "0123456789abcdef0123456789abcdef"
        );
        assert!(parse_ssh_credential("nope").is_err());
        assert!(parse_ssh_credential("credential short").is_err());
    }
}
