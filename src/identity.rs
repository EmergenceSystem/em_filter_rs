//! Agent identity: an ed25519 keypair plus the signed wire payloads shared by
//! both mesh transports (Model A `server`, Model B `wsclient`).

use serde_json::{Value, json};
use std::path::Path;

use crate::crypto;

/// An agent's ed25519 keypair, name and capabilities.
///
/// Builds the self-signed handshake and gossip payloads, and signs outgoing
/// result lists — all identical to the byte forms produced by
/// `em_pop_crypto.erl` (see [`crate::crypto`]).
pub struct Identity {
    /// Agent name, sent in the `hello` / gossip payloads.
    pub name: String,
    /// Capabilities advertised to disco (routing hints only — see
    /// `docs/superpowers/specs/2026-09-04-multilang-sdk-mesh-parity-design.md` §5).
    pub capabilities: Vec<String>,
    /// 32-byte ed25519 public key.
    pub pubkey: [u8; 32],
    /// 32-byte ed25519 seed (private key material).
    pub seed: [u8; 32],
    /// Peer id — `SHA-256(pubkey)[0:16]`.
    pub id: [u8; 16],
}

impl Identity {
    /// Load or create the keypair under `key_dir` (see [`crypto::load_or_create`])
    /// and derive the peer id.
    pub fn new(
        name: impl Into<String>,
        key_dir: impl AsRef<Path>,
        capabilities: Vec<String>,
    ) -> std::io::Result<Self> {
        let (pubkey, seed) = crypto::load_or_create(key_dir)?;
        let id = crypto::id_of(&pubkey);
        Ok(Self {
            name: name.into(),
            capabilities,
            pubkey,
            seed,
            id,
        })
    }

    /// `base64(Ed25519_sign(canonical_identity(id, name), seed))`.
    fn selfsig_b64(&self) -> String {
        let sig = crypto::sign(&crypto::canonical_identity(&self.id, &self.name), &self.seed);
        crypto::base64_encode(&sig)
    }

    /// Build the Model B `hello` handshake frame.
    pub fn hello_payload(&self) -> Value {
        json!({
            "action": "hello",
            "name": self.name,
            "pubkey": crypto::base64_encode(&self.pubkey),
            "sig": self.selfsig_b64(),
            "capabilities": self.capabilities,
        })
    }

    /// Build the Model A gossip self-payload advertised to disco seeds.
    pub fn gossip_payload(&self, host: &str, query_port: u16) -> Value {
        json!({
            "id": crypto::base64_encode(&self.id),
            "name": self.name,
            "host": host,
            "query_port": query_port,
            "pubkey": crypto::base64_encode(&self.pubkey),
            "sig": self.selfsig_b64(),
            "capabilities": self.capabilities,
            "role": "filter",
        })
    }

    /// Sign a result list, returning `(signer_id_b64, signature_b64)`.
    ///
    /// `signer_id = base64(id)`, `signature = base64(Ed25519_sign(canonical_response(items), seed))`.
    pub fn sign_results(&self, items: &Value) -> (String, String) {
        let sig = crypto::sign(&crypto::canonical_response(items), &self.seed);
        (crypto::base64_encode(&self.id), crypto::base64_encode(&sig))
    }

    /// Sign a result list with the v2 canonical form, binding the response to
    /// `query` and a fresh timestamp. Returns `(ts_ms, signer_id_b64, signature_b64)`.
    pub fn sign_results_v2(&self, query: &str, items: &Value) -> (i64, String, String) {
        let ts = now_ms();
        let (signer_id, signature) =
            crypto::sign_response_v2(query, ts, items, &self.pubkey, &self.seed);
        (ts, signer_id, signature)
    }

    /// Build the `x-pop-*` auth headers for a gossip POST carrying exactly `body`:
    /// `x-pop-id`, `x-pop-ts` (unix millis) and `x-pop-sig`.
    pub fn gossip_headers(&self, body: &[u8]) -> Vec<(String, String)> {
        let ts = now_ms();
        vec![
            ("x-pop-id".to_string(), crypto::base64_encode(&self.id)),
            ("x-pop-ts".to_string(), ts.to_string()),
            (
                "x-pop-sig".to_string(),
                crypto::sign_gossip(&self.id, ts, body, &self.seed),
            ),
        ]
    }
}

/// Current unix time in milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("em_filter_rs_identity_{tag}_{}", std::process::id()))
    }

    #[test]
    fn hello_payload_has_expected_shape() {
        let dir = tmp_dir("hello");
        let _ = std::fs::remove_dir_all(&dir);
        let id = Identity::new("test_agent", &dir, vec!["search".into(), "query".into()]).unwrap();

        let hello = id.hello_payload();
        assert_eq!(hello["action"], "hello");
        assert_eq!(hello["name"], "test_agent");
        assert!(hello["pubkey"].as_str().unwrap().len() > 0);
        assert!(hello["sig"].as_str().unwrap().len() > 0);
        assert_eq!(hello["capabilities"], json!(["search", "query"]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gossip_payload_has_expected_shape() {
        let dir = tmp_dir("gossip");
        let _ = std::fs::remove_dir_all(&dir);
        let id = Identity::new("test_agent", &dir, vec!["search".into()]).unwrap();

        let payload = id.gossip_payload("1.2.3.4", 9600);
        assert_eq!(payload["name"], "test_agent");
        assert_eq!(payload["host"], "1.2.3.4");
        assert_eq!(payload["query_port"], 9600);
        assert_eq!(payload["role"], "filter");
        assert!(payload["id"].as_str().unwrap().len() > 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sign_results_verifies_against_pubkey() {
        let dir = tmp_dir("sign");
        let _ = std::fs::remove_dir_all(&dir);
        let id = Identity::new("test_agent", &dir, vec![]).unwrap();

        let items = json!([{"type": "url", "properties": {"url": "https://example.com", "title": "t"}}]);
        let (signer_id, signature) = id.sign_results(&items);

        assert_eq!(signer_id, crypto::base64_encode(&id.id));
        let sig_bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &signature).unwrap();
        assert!(crypto::verify(&crypto::canonical_response(&items), &sig_bytes, &id.pubkey));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sign_results_v2_verifies_against_pubkey() {
        let dir = tmp_dir("sign_v2");
        let _ = std::fs::remove_dir_all(&dir);
        let id = Identity::new("test_agent", &dir, vec![]).unwrap();

        let items = json!([{"type": "url", "properties": {"url": "https://example.com"}}]);
        let (ts, signer_id, signature) = id.sign_results_v2("erlang", &items);

        assert!(ts > 0);
        assert_eq!(signer_id, crypto::base64_encode(&id.id));
        let sig = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &signature).unwrap();
        assert!(crypto::verify(&crypto::canonical_response_v2("erlang", ts, &items), &sig, &id.pubkey));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gossip_headers_verify_against_pubkey() {
        let dir = tmp_dir("gossip_hdr");
        let _ = std::fs::remove_dir_all(&dir);
        let id = Identity::new("test_agent", &dir, vec![]).unwrap();

        let body = br#"{"id":"x"}"#;
        let hdrs: std::collections::HashMap<_, _> = id.gossip_headers(body).into_iter().collect();

        assert_eq!(hdrs["x-pop-id"], crypto::base64_encode(&id.id));
        let ts: i64 = hdrs["x-pop-ts"].parse().unwrap();
        let sig = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &hdrs["x-pop-sig"]).unwrap();
        let hash = <sha2::Sha256 as sha2::Digest>::digest(body);
        assert!(crypto::verify(&crypto::canonical_gossip_auth(&id.id, ts, &hash), &sig, &id.pubkey));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
