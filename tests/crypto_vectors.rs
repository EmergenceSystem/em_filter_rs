//! Fixture-driven test that `em_filter::crypto` reproduces the Erlang reference
//! implementation (`em_pop_crypto.erl`) byte-for-byte: `id_of`, `canonical_identity`,
//! `canonical_response`, and ed25519 signatures over both canonical forms.

use em_filter::crypto;
use serde_json::Value;

#[derive(serde::Deserialize)]
struct Fixture {
    canonical_identity_hex: String,
    canonical_response_hex: String,
    id_hex: String,
    items: Value,
    name: String,
    privkey_hex: String,
    pubkey_hex: String,
    response_signature_b64: String,
    selfsig_b64: String,
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
fn crypto_vectors_match_erlang_reference() {
    let fx = load_fixture();

    let pubkey = hex::decode(&fx.pubkey_hex).expect("pubkey_hex is valid hex");
    let seed = hex::decode(&fx.privkey_hex).expect("privkey_hex is valid hex");
    assert_eq!(pubkey.len(), 32, "pubkey must be 32 bytes");
    assert_eq!(seed.len(), 32, "seed must be 32 bytes");

    // 1. id_of
    let id = crypto::id_of(&pubkey);
    assert_eq!(hex::encode(id), fx.id_hex, "id_of hex mismatch");
    assert_eq!(
        crypto::base64_encode(&id),
        fx.signer_id_b64,
        "id_of base64 mismatch"
    );

    // 2. canonical_identity
    let canon_identity = crypto::canonical_identity(&id, &fx.name);
    assert_eq!(
        hex::encode(&canon_identity),
        fx.canonical_identity_hex,
        "canonical_identity hex mismatch"
    );

    // 3. self-signature over canonical_identity
    let selfsig = crypto::sign(&canon_identity, &seed);
    assert_eq!(
        crypto::base64_encode(&selfsig),
        fx.selfsig_b64,
        "self-signature mismatch"
    );

    // 4. canonical_response
    let canon_response = crypto::canonical_response(&fx.items);
    assert_eq!(
        hex::encode(&canon_response),
        fx.canonical_response_hex,
        "canonical_response hex mismatch"
    );

    // 5. response signature
    let response_sig = crypto::sign(&canon_response, &seed);
    assert_eq!(
        crypto::base64_encode(&response_sig),
        fx.response_signature_b64,
        "response signature mismatch"
    );

    // 6. verify the response signature against the pubkey
    assert!(
        crypto::verify(&canon_response, &response_sig, &pubkey),
        "response signature must verify against pubkey"
    );
}

#[test]
fn load_or_create_roundtrips_key_file() {
    let dir = std::env::temp_dir().join(format!("em_filter_rs_smoke_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (pub1, seed1) = crypto::load_or_create(&dir).expect("create keypair");
    assert!(dir.join("node_ed25519.key").exists());
    let (pub2, seed2) = crypto::load_or_create(&dir).expect("load keypair");
    assert_eq!(pub1, pub2);
    assert_eq!(seed1, seed2);
    let _ = std::fs::remove_dir_all(&dir);
}
