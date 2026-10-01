//! Fixture-driven byte-parity test for the v2 response signature (query + timestamp
//! bound) and the gossip-auth signature, shared across all Emergence SDKs.

use em_filter::crypto;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(serde::Deserialize)]
struct Fixture {
    v2_query: String,
    v2_ts: i64,
    canonical_response_v2_hex: String,
    response_v2_signature_b64: String,
    gossip_body_utf8: String,
    gossip_body_sha256_hex: String,
    gossip_ts: i64,
    canonical_gossip_auth_hex: String,
    gossip_signature_b64: String,
    privkey_hex: String,
    pubkey_hex: String,
    id_hex: String,
    items: Value,
    signer_id_b64: String,
}

fn load_fixture() -> Fixture {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/crypto_vectors.json"
    ))
    .expect("failed to read fixtures/crypto_vectors.json");
    serde_json::from_str(&raw).expect("failed to parse fixtures/crypto_vectors.json")
}

#[test]
fn response_v2_matches_fixture() {
    let fx = load_fixture();
    let pubkey = hex::decode(&fx.pubkey_hex).unwrap();
    let seed = hex::decode(&fx.privkey_hex).unwrap();

    let canon = crypto::canonical_response_v2(&fx.v2_query, fx.v2_ts, &fx.items);
    assert_eq!(
        hex::encode(&canon),
        fx.canonical_response_v2_hex,
        "canonical_response_v2 hex mismatch"
    );

    let sig = crypto::sign(&canon, &seed);
    assert_eq!(
        crypto::base64_encode(&sig),
        fx.response_v2_signature_b64,
        "v2 response signature mismatch"
    );
    assert!(
        crypto::verify(&canon, &sig, &pubkey),
        "v2 response signature must verify"
    );

    let (signer_id_b64, sig_b64) =
        crypto::sign_response_v2(&fx.v2_query, fx.v2_ts, &fx.items, &pubkey, &seed);
    assert_eq!(signer_id_b64, fx.signer_id_b64);
    assert_eq!(sig_b64, fx.response_v2_signature_b64);
}

#[test]
fn gossip_auth_matches_fixture() {
    let fx = load_fixture();
    let pubkey = hex::decode(&fx.pubkey_hex).unwrap();
    let seed = hex::decode(&fx.privkey_hex).unwrap();
    let id = hex::decode(&fx.id_hex).unwrap();
    let body = fx.gossip_body_utf8.as_bytes();

    let body_hash = Sha256::digest(body);
    assert_eq!(
        hex::encode(body_hash),
        fx.gossip_body_sha256_hex,
        "gossip body sha256 mismatch"
    );

    let canon = crypto::canonical_gossip_auth(&id, fx.gossip_ts, &body_hash);
    assert_eq!(
        hex::encode(&canon),
        fx.canonical_gossip_auth_hex,
        "canonical_gossip_auth hex mismatch"
    );

    let sig_b64 = crypto::sign_gossip(&id, fx.gossip_ts, body, &seed);
    assert_eq!(sig_b64, fx.gossip_signature_b64, "gossip signature mismatch");

    use base64::Engine;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(&sig_b64)
        .expect("valid base64");
    assert!(
        crypto::verify(&canon, &sig, &pubkey),
        "gossip signature must verify"
    );
}
