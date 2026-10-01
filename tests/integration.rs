//! Integration tests — requires a live tokio runtime and spawns a mock relay
//! (`em_disco`'s `/ws/filter` endpoint) speaking the Model B protocol:
//! `hello` -> `hello_ok`, `query` -> `result`.

use em_filter::async_trait;
use em_filter::EmFilterError;
use em_filter::{AgentConfig, DiscoNode, Filter, FilterRunner};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio_tungstenite::{accept_async, tungstenite::Message};

/// Counter filter — increments a shared counter on each query and echoes the body.
struct CountFilter {
    count: Arc<AtomicUsize>,
}

#[async_trait]
impl Filter for CountFilter {
    async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
        let n = self.count.fetch_add(1, Ordering::SeqCst);
        Ok(json!([{
            "type": "url",
            "properties": { "url": "https://example.com", "title": format!("{n}:{body}") }
        }]))
    }
}

#[tokio::test]
async fn test_filterrunner_relay_handshake_and_query_dispatch() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Value>();

    // Spawn mock relay (em_disco's /ws/filter) server.
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws.split();

        // Receive hello frame, assert its shape.
        let msg = read.next().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        assert_eq!(v["action"], "hello");
        assert_eq!(v["name"], "integration_agent");
        assert!(v["pubkey"].as_str().is_some());
        assert!(v["sig"].as_str().is_some());
        assert!(v["capabilities"].is_array());

        // Ack.
        write
            .send(Message::Text(
                json!({"action": "hello_ok", "id": "test-disco-id"}).to_string().into(),
            ))
            .await
            .unwrap();

        // Send a query.
        write
            .send(Message::Text(
                json!({"action": "query", "id": "int-1", "body": "hello world"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();

        // Receive the signed result.
        let msg = read.next().await.unwrap().unwrap();
        let mut result: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        // Hand the hello pubkey to the test so it can verify the signature.
        result["_pubkey"] = v["pubkey"].clone();
        let _ = result_tx.send(result);
    });

    let count = Arc::new(AtomicUsize::new(0));
    let filter = CountFilter { count: count.clone() };
    let config = AgentConfig {
        disco_nodes: vec![DiscoNode { host: "127.0.0.1".into(), port, tls: false }],
        jwt_token: None,
    };

    let key_dir = std::env::temp_dir().join(format!(
        "em_filter_rs_it_relay_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&key_dir);
    unsafe {
        std::env::set_var("EM_FILTER_KEY_DIR", &key_dir);
        std::env::set_var("EM_FILTER_MODE", "relay");
    }

    let runner_handle =
        tokio::spawn(FilterRunner::new("integration_agent", filter, config).run());
    tokio::time::sleep(Duration::from_secs(3)).await;
    runner_handle.abort();

    let result = tokio::time::timeout(Duration::from_secs(3), result_rx)
        .await
        .expect("timeout waiting for result")
        .expect("channel closed");

    assert_eq!(result["action"], "result");
    assert_eq!(result["id"], "int-1");
    assert!(result["signer_id"].as_str().is_some());
    assert_v2_signature(&result, "hello world");
    assert_eq!(
        result["results"][0]["properties"]["title"],
        "0:hello world"
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);

    let _ = std::fs::remove_dir_all(&key_dir);
}

#[tokio::test]
async fn test_agent_server_signs_query_results() {
    use em_filter::{AgentServer, Identity};
    use tokio::sync::Mutex;

    let key_dir = std::env::temp_dir().join(format!(
        "em_filter_rs_it_direct_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&key_dir);

    let identity = Arc::new(
        Identity::new("direct_agent", &key_dir, vec!["search".into()]).unwrap(),
    );
    let pubkey = identity.pubkey;
    let count = Arc::new(AtomicUsize::new(0));
    let filter = Arc::new(Mutex::new(CountFilter { count: count.clone() }));

    let server = AgentServer::bind(identity.clone(), filter, "127.0.0.1", 0, "127.0.0.1")
        .expect("bind AgentServer");
    let port = server.port;
    let _thread = server.start();

    // Give the server a moment to enter its accept loop.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let body = tokio::task::spawn_blocking(move || {
        http_post(port, "/agent/query", r#"{"query":"erlang"}"#)
    })
    .await
    .unwrap();

    let v: Value = serde_json::from_str(&body).expect("valid JSON response");
    assert!(v["signer_id"].as_str().is_some());
    let mut v = v;
    v["_pubkey"] = json!(em_filter::crypto::base64_encode(&pubkey));
    assert_v2_signature(&v, "erlang");
    assert_eq!(v["results"][0]["properties"]["title"], "0:erlang");

    let _ = std::fs::remove_dir_all(&key_dir);
}

/// Assert `resp` carries a `ts` and a v2 signature over `(query, ts, results)`
/// that verifies under `resp["_pubkey"]` (base64, injected by the test).
fn assert_v2_signature(resp: &Value, query: &str) {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let ts = resp["ts"].as_i64().expect("response carries ts");
    assert!(ts > 0);
    let pubkey = STANDARD.decode(resp["_pubkey"].as_str().unwrap()).unwrap();
    let sig = STANDARD.decode(resp["signature"].as_str().unwrap()).unwrap();
    let canon = em_filter::crypto::canonical_response_v2(query, ts, &resp["results"]);
    assert!(em_filter::crypto::verify(&canon, &sig, &pubkey), "v2 signature must verify");
}

/// Minimal blocking HTTP/1.1 POST used only to exercise `AgentServer` in tests.
fn http_post(port: u16, path: &str, body: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        path = path,
        len = body.len(),
        body = body,
    );
    stream.write_all(req.as_bytes()).unwrap();

    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
}

#[test]
fn test_gossip_pusher_sends_signed_headers() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use em_filter::{GossipPusher, Identity};
    use std::io::Read;

    let key_dir = std::env::temp_dir().join(format!("em_filter_rs_it_gossip_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&key_dir);
    let identity = Arc::new(Identity::new("gossip_agent", &key_dir, vec![]).unwrap());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let pusher = GossipPusher::new(
        identity.clone(),
        vec![("127.0.0.1".into(), port)],
        "127.0.0.1",
        9600,
        Duration::from_millis(200),
    )
    .start();

    let (mut stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    // Read until headers + full body (Content-Length) are in.
    loop {
        let n = stream.read(&mut buf).unwrap();
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some(idx) = text.find("\r\n\r\n") {
            let len: usize = text[..idx]
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                .unwrap();
            if raw.len() >= idx + 4 + len {
                break;
            }
        }
        if n == 0 {
            break;
        }
    }
    drop(stream);
    pusher.stop();

    let text = String::from_utf8(raw).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let header = |name: &str| -> String {
        head.lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
            .unwrap_or_else(|| panic!("missing header {name}"))
    };

    assert_eq!(header("x-pop-id"), em_filter::crypto::base64_encode(&identity.id));
    let ts: i64 = header("x-pop-ts").parse().unwrap();
    let sig = STANDARD.decode(header("x-pop-sig")).unwrap();
    let hash = <sha2::Sha256 as sha2::Digest>::digest(body.as_bytes());
    let canon = em_filter::crypto::canonical_gossip_auth(&identity.id, ts, &hash);
    assert!(em_filter::crypto::verify(&canon, &sig, &identity.pubkey));

    let _ = std::fs::remove_dir_all(&key_dir);
}
