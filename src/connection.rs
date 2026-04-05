use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use crate::{Filter, DiscoNode, EmFilterError};

/// Manages a single persistent WebSocket connection to one em_disco node.
///
/// Runs forever, reconnecting after any error or clean disconnect.
/// Shares the filter instance with other connections via `Arc<Mutex<F>>`.
pub(crate) struct Connection<F: Filter> {
    /// Agent name sent in the `register` frame.
    name: String,
    /// The em_disco node to connect to.
    node: DiscoNode,
    /// Shared filter instance — one lock acquisition per query.
    filter: Arc<Mutex<F>>,
    /// Optional JWT token appended as `?token=<jwt>` to the WebSocket URL.
    jwt_token: Option<String>,
}

impl<F: Filter> Connection<F> {
    /// Create a new connection.
    pub(crate) fn new(
        name: String,
        node: DiscoNode,
        filter: Arc<Mutex<F>>,
        jwt_token: Option<String>,
    ) -> Self {
        Self { name, node, filter, jwt_token }
    }

    /// Run the connection loop forever, reconnecting on any error or close.
    pub(crate) async fn run(self) {
        let delay = reconnect_delay();
        loop {
            match self.connect_once().await {
                Ok(()) => {
                    tracing::info!(agent = %self.name, "Disconnected from em_disco, reconnecting");
                }
                Err(e) => {
                    tracing::warn!(
                        agent = %self.name,
                        host  = %self.node.host,
                        port  = self.node.port,
                        error = %e,
                        "Connection error, reconnecting"
                    );
                }
            }
            tokio::time::sleep(delay).await;
        }
    }

    /// Open one WebSocket connection: handshake -> message loop -> return on close/error.
    async fn connect_once(&self) -> Result<(), EmFilterError> {
        let url = self.ws_url();
        tracing::info!(agent = %self.name, url = %url, "Connecting to em_disco");

        let (ws, _) = connect_async(&url).await?;
        let (mut write, mut read) = ws.split();

        // Step 1: announce agent name.
        write
            .send(Message::Text(
                json!({"action": "register", "name": &self.name}).to_string(),
            ))
            .await?;

        // Step 2: announce capabilities.
        // The Erlang library sends both frames without waiting for acks.
        let caps = self.filter.lock().await.capabilities();
        write
            .send(Message::Text(
                json!({"action": "agent_hello", "capabilities": caps}).to_string(),
            ))
            .await?;

        tracing::info!(
            agent = %self.name,
            host  = %self.node.host,
            port  = self.node.port,
            "Registered on em_disco — entering message loop"
        );

        // Message loop: handle query frames, ignore everything else.
        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(text) => {
                    let v: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => {
                            tracing::warn!(agent = %self.name, "Invalid JSON from disco, ignoring");
                            continue;
                        }
                    };

                    if v["action"] == "query" {
                        let id = v["id"].as_str().unwrap_or("").to_string();
                        let body = v["body"].as_str().unwrap_or("").to_string();

                        tracing::info!(
                            agent    = %self.name,
                            query_id = %id,
                            body     = %body,
                            "Handling query"
                        );

                        // Acquire lock, call handler, release before sending.
                        let result = {
                            let mut f = self.filter.lock().await;
                            match f.handle(&body).await {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!(
                                        agent    = %self.name,
                                        query_id = %id,
                                        error    = %e,
                                        "Filter handler returned error"
                                    );
                                    json!(null)
                                }
                            }
                        };

                        write
                            .send(Message::Text(
                                json!({
                                    "action": "result",
                                    "id":     id,
                                    "data":   result
                                })
                                .to_string(),
                            ))
                            .await?;
                    }
                    // registered / agent_registered acks are silently ignored,
                    // identical to the Erlang em_filter_server behaviour.
                }
                Message::Close(_) => {
                    tracing::info!(agent = %self.name, "em_disco closed the connection");
                    return Ok(());
                }
                // Ping / Pong / Binary frames are ignored.
                _ => {}
            }
        }

        Ok(())
    }

    /// Build the WebSocket URL for this node.
    ///
    /// Appends `?token=<jwt>` when a token is configured.
    fn ws_url(&self) -> String {
        let scheme = if self.node.tls { "wss" } else { "ws" };
        match &self.jwt_token {
            Some(token) => format!(
                "{}://{}:{}/ws?token={}",
                scheme, self.node.host, self.node.port, token
            ),
            None => format!("{}://{}:{}/ws", scheme, self.node.host, self.node.port),
        }
    }
}

/// Reconnect delay in milliseconds.
///
/// Reads `EM_FILTER_RECONNECT_MS` from the environment; defaults to 5000.
fn reconnect_delay() -> Duration {
    let ms: u64 = std::env::var("EM_FILTER_RECONNECT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Filter;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_tungstenite::accept_async;

    /// A simple echo filter used in tests.
    struct EchoFilter;

    #[async_trait]
    impl Filter for EchoFilter {
        async fn handle(&mut self, body: &str) -> Result<Value, crate::EmFilterError> {
            Ok(json!({"echo": body}))
        }
    }

    /// Spawn a mock em_disco WebSocket server on a random port.
    ///
    /// The server:
    ///   1. Accepts the connection.
    ///   2. Reads the `register` frame and asserts the agent name.
    ///   3. Reads the `agent_hello` frame and asserts it has `capabilities`.
    ///   4. Sends one query frame.
    ///   5. Reads the result frame and returns it via a oneshot channel.
    async fn spawn_mock_disco(
        expected_name: &'static str,
        query_body: &'static str,
    ) -> (u16, tokio::sync::oneshot::Receiver<Value>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = accept_async(stream).await.unwrap();
            let (mut write, mut read) = ws.split();

            // Receive register frame
            let msg = read.next().await.unwrap().unwrap();
            let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            assert_eq!(v["action"], "register", "first frame must be register");
            assert_eq!(v["name"], expected_name);

            // Receive agent_hello frame
            let msg = read.next().await.unwrap().unwrap();
            let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            assert_eq!(v["action"], "agent_hello", "second frame must be agent_hello");
            assert!(v["capabilities"].is_array());

            // Send a query
            write
                .send(Message::Text(
                    json!({"action": "query", "id": "test-q1", "body": query_body})
                        .to_string(),
                ))
                .await
                .unwrap();

            // Receive result
            let msg = read.next().await.unwrap().unwrap();
            let result: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let _ = tx.send(result);
        });

        (port, rx)
    }

    #[tokio::test]
    async fn test_handshake_and_query_dispatch() {
        let (port, result_rx) = spawn_mock_disco("test_agent", "erlang otp").await;

        let filter = Arc::new(Mutex::new(EchoFilter));
        let node = DiscoNode {
            host: "127.0.0.1".into(),
            port,
            tls: false,
        };
        let conn = Connection::new("test_agent".into(), node, filter, None);

        // Run with a short timeout — the mock server will close after sending the result,
        // causing connect_once to return Ok(()), which triggers the reconnect path in run().
        let _ = tokio::time::timeout(Duration::from_secs(3), conn.run()).await;

        let result = result_rx.await.expect("mock disco did not send result");
        assert_eq!(result["action"], "result");
        assert_eq!(result["id"], "test-q1");
        assert_eq!(result["data"]["echo"], "erlang otp");
    }

    #[test]
    fn test_ws_url_plain() {
        let filter = Arc::new(Mutex::new(EchoFilter));
        let node = DiscoNode { host: "localhost".into(), port: 8080, tls: false };
        let conn = Connection::new("a".into(), node, filter, None);
        assert_eq!(conn.ws_url(), "ws://localhost:8080/ws");
    }

    #[test]
    fn test_ws_url_tls_with_token() {
        let filter = Arc::new(Mutex::new(EchoFilter));
        let node = DiscoNode { host: "disco.example.com".into(), port: 443, tls: true };
        let conn = Connection::new("a".into(), node, filter, Some("mytoken".into()));
        assert_eq!(conn.ws_url(), "wss://disco.example.com:443/ws?token=mytoken");
    }
}
