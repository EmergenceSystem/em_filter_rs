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
        let result: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
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
    assert!(result["signature"].as_str().is_some());
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
    assert!(v["signature"].as_str().is_some());
    assert_eq!(v["results"][0]["properties"]["title"], "0:erlang");

    let _ = std::fs::remove_dir_all(&key_dir);
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
