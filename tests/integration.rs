//! Integration tests — requires a live tokio runtime and spawns a mock em_disco server.

use em_filter::{AgentConfig, DiscoNode, Filter, FilterRunner};
use em_filter::async_trait;
use em_filter::EmFilterError;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
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
        Ok(json!({"count": n, "body": body}))
    }
}

#[tokio::test]
async fn test_filterrunner_connects_and_handles_query() {
    // Bind on a random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Value>();

    // Spawn mock em_disco server
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws.split();

        // Consume register frame
        let msg = read.next().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        assert_eq!(v["action"], "register");
        assert_eq!(v["name"], "integration_agent");

        // Consume agent_hello frame
        let msg = read.next().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        assert_eq!(v["action"], "agent_hello");

        // Send query
        write
            .send(Message::Text(
                json!({"action": "query", "id": "int-1", "body": "hello world"}).to_string().into(),
            ))
            .await
            .unwrap();

        // Receive result
        let msg = read.next().await.unwrap().unwrap();
        let result: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        let _ = result_tx.send(result);
    });

    // Set up runner pointing at the mock server
    let count = Arc::new(AtomicUsize::new(0));
    let filter = CountFilter { count: count.clone() };
    let config = AgentConfig {
        disco_nodes: vec![DiscoNode {
            host: "127.0.0.1".into(),
            port,
            tls: false,
        }],
        jwt_token: None,
    };

    // Spawn the runner and abort it after the timeout to prevent background task leakage.
    let runner_handle = tokio::spawn(FilterRunner::new("integration_agent", filter, config).run());
    tokio::time::sleep(Duration::from_secs(3)).await;
    runner_handle.abort();

    // Verify the result received by the mock server
    let result = tokio::time::timeout(Duration::from_secs(3), result_rx)
        .await
        .expect("timeout waiting for result")
        .expect("channel closed");

    assert_eq!(result["action"], "result");
    assert_eq!(result["id"], "int-1");
    assert_eq!(result["data"]["body"], "hello world");
    assert_eq!(result["data"]["count"], 0);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_filterrunner_shared_state_across_queries() {
    // Two sequential queries — verify count increments correctly.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx1, rx1) = tokio::sync::oneshot::channel::<Value>();
    let (tx2, rx2) = tokio::sync::oneshot::channel::<Value>();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws = accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws.split();

        // Skip register + agent_hello
        read.next().await;
        read.next().await;

        // Query 1
        write
            .send(Message::Text(
                json!({"action": "query", "id": "q1", "body": "first"}).to_string().into(),
            ))
            .await
            .unwrap();
        let msg = read.next().await.unwrap().unwrap();
        let _ = tx1.send(serde_json::from_str(msg.to_text().unwrap()).unwrap());

        // Query 2
        write
            .send(Message::Text(
                json!({"action": "query", "id": "q2", "body": "second"}).to_string().into(),
            ))
            .await
            .unwrap();
        let msg = read.next().await.unwrap().unwrap();
        let _ = tx2.send(serde_json::from_str(msg.to_text().unwrap()).unwrap());
    });

    let count = Arc::new(AtomicUsize::new(0));
    let filter = CountFilter { count: count.clone() };
    let config = AgentConfig {
        disco_nodes: vec![DiscoNode {
            host: "127.0.0.1".into(),
            port,
            tls: false,
        }],
        jwt_token: None,
    };

    let runner_handle = tokio::spawn(FilterRunner::new("shared_state_agent", filter, config).run());
    tokio::time::sleep(Duration::from_secs(3)).await;
    runner_handle.abort();

    let r1 = tokio::time::timeout(Duration::from_secs(3), rx1).await.unwrap().unwrap();
    let r2 = tokio::time::timeout(Duration::from_secs(3), rx2).await.unwrap().unwrap();

    // First query gets count=0, second gets count=1 — state is shared and incremented
    assert_eq!(r1["data"]["count"], 0);
    assert_eq!(r1["data"]["body"], "first");
    assert_eq!(r2["data"]["count"], 1);
    assert_eq!(r2["data"]["body"], "second");
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
