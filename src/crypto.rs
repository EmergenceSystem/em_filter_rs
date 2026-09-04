//! Ed25519 crypto primitives for the Emergence signed mesh.
//!
//! This module mirrors the Erlang reference implementation, `em_pop_crypto.erl`,
//! byte-for-byte. All canonical byte forms and signatures produced here must be
//! identical to what the Erlang node produces, since both sides of the mesh
//! verify each other's signatures.

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;

/// Name of the key file written by [`load_or_create`] inside a given key directory.
const KEY_FILE_NAME: &str = "node_ed25519.key";

/// Derive the 16-byte node id from a 32-byte ed25519 public key.
///
/// `id_of(pubkey) = SHA-256(pubkey)[0:16]` — the first 16 raw bytes of the
/// SHA-256 digest of the public key.
pub fn id_of(pubkey: &[u8]) -> [u8; 16] {
    let digest = Sha256::digest(pubkey);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[0..16]);
    id
}

/// Build the canonical "identity" byte form: `id ‖ 0x00 ‖ name_utf8_bytes`.
pub fn canonical_identity(id: &[u8], name: &str) -> Vec<u8> {
    let name_bytes = name.as_bytes();
    let mut out = Vec::with_capacity(id.len() + 1 + name_bytes.len());
    out.extend_from_slice(id);
    out.push(0x00);
    out.extend_from_slice(name_bytes);
    out
}

/// Extract the first string value found among `keys` in the given JSON object,
/// or `""` if none of them are present as strings.
fn first_string<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> &'a str {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            if let Some(s) = v.as_str() {
                return s;
            }
        }
    }
    ""
}

/// Build the canonical "response" byte form for a list of result items.
///
/// For each item (in list order) one line is emitted:
/// `U ‖ 0x00 ‖ T ‖ 0x00 ‖ R ‖ 0x0A`
///
/// Where, given `P` = `item.properties` (if present and an object) else `item`
/// itself (if `item` is an object; otherwise there is no `P` and all fields are
/// empty):
/// - `U` = `P.url` if a string, else `""`
/// - `T` = first string among `P.title`, `P.label`, else `""`
/// - `R` = first string among `P.resume`, `P.value`, `P.description`, else `""`
///
/// If `items` is not a JSON array, the canonical response is empty.
pub fn canonical_response(items: &serde_json::Value) -> Vec<u8> {
    let Some(array) = items.as_array() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for item in array {
        let obj = item.as_object();

        // P = item.properties if present and an object, else item itself
        // (if item is an object at all).
        let props: Option<&serde_json::Map<String, serde_json::Value>> = match obj {
            Some(o) => match o.get("properties").and_then(|v| v.as_object()) {
                Some(p) => Some(p),
                None => Some(o),
            },
            None => None,
        };

        let (u, t, r): (&str, &str, &str) = match props {
            Some(p) => {
                let u = p.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let t = first_string(p, &["title", "label"]);
                let r = first_string(p, &["resume", "value", "description"]);
                (u, t, r)
            }
            None => ("", "", ""),
        };

        out.extend_from_slice(u.as_bytes());
        out.push(0x00);
        out.extend_from_slice(t.as_bytes());
        out.push(0x00);
        out.extend_from_slice(r.as_bytes());
        out.push(0x0A);
    }

    out
}

/// Sign `msg` with the ed25519 seed (32 raw bytes — *not* a 64-byte secret key).
///
/// # Panics
/// Panics if `seed` is not exactly 32 bytes long.
pub fn sign(msg: &[u8], seed: &[u8]) -> [u8; 64] {
    let seed_arr: [u8; 32] = seed
        .try_into()
        .expect("ed25519 seed must be exactly 32 bytes");
    let signing_key = SigningKey::from_bytes(&seed_arr);
    signing_key.sign(msg).to_bytes()
}

/// Verify `sig` against `msg` under `pubkey`. Never panics — returns `false`
/// for malformed input (wrong-length key/signature) or a failed verification.
pub fn verify(msg: &[u8], sig: &[u8], pubkey: &[u8]) -> bool {
    let Ok(pubkey_arr): Result<[u8; 32], _> = pubkey.try_into() else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; 64], _> = sig.try_into() else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&pubkey_arr) else {
        return false;
    };
    let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);
    verifying_key.verify(msg, &signature).is_ok()
}

/// Load the node's ed25519 keypair from `key_dir/node_ed25519.key`, creating one
/// if it doesn't exist yet.
///
/// The key file layout is raw bytes: `pubkey(32) ‖ seed(32)` — 64 bytes total.
/// Returns `(pubkey, seed)`.
pub fn load_or_create(key_dir: impl AsRef<Path>) -> io::Result<([u8; 32], [u8; 32])> {
    let key_dir = key_dir.as_ref();
    let key_path = key_dir.join(KEY_FILE_NAME);

    if key_path.exists() {
        let bytes = std::fs::read(&key_path)?;
        if bytes.len() != 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "key file {} has invalid length {} (expected 64)",
                    key_path.display(),
                    bytes.len()
                ),
            ));
        }
        let mut pubkey = [0u8; 32];
        let mut seed = [0u8; 32];
        pubkey.copy_from_slice(&bytes[0..32]);
        seed.copy_from_slice(&bytes[32..64]);
        Ok((pubkey, seed))
    } else {
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = signing_key.verifying_key().to_bytes();
        let seed = signing_key.to_bytes();

        if let Some(parent) = key_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file_bytes = Vec::with_capacity(64);
        file_bytes.extend_from_slice(&pubkey);
        file_bytes.extend_from_slice(&seed);
        std::fs::write(&key_path, &file_bytes)?;

        Ok((pubkey, seed))
    }
}

/// Base64-encode with standard padded alphabet — matches Erlang's `base64:encode/1`.
pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_of_is_16_bytes_of_sha256() {
        let pubkey = [1u8; 32];
        let expected = Sha256::digest(pubkey);
        let id = id_of(&pubkey);
        assert_eq!(&id[..], &expected[0..16]);
    }

    #[test]
    fn canonical_identity_layout() {
        let id = [0xAAu8; 4];
        let out = canonical_identity(&id, "n");
        assert_eq!(out, vec![0xAA, 0xAA, 0xAA, 0xAA, 0x00, b'n']);
    }

    #[test]
    fn canonical_response_non_array_is_empty() {
        let v = serde_json::json!({"not": "an array"});
        assert!(canonical_response(&v).is_empty());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let seed = [7u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let pubkey = signing_key.verifying_key().to_bytes();
        let msg = b"hello world";
        let sig = sign(msg, &seed);
        assert!(verify(msg, &sig, &pubkey));
        assert!(!verify(b"tampered", &sig, &pubkey));
    }

    #[test]
    fn verify_handles_malformed_input_without_panic() {
        assert!(!verify(b"msg", &[0u8; 3], &[0u8; 32]));
        assert!(!verify(b"msg", &[0u8; 64], &[0u8; 3]));
    }
}
