//! Model B (relay, default): an outbound WebSocket connection to
//! `wss://<disco>/ws/filter`, speaking `hello` / `hello_ok` / `query` /
//! `result`. Never needs inbound reachability. Mirrors the Python reference,
//! `em_filter/wsclient.py`'s `RelayClient`.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{EmFilterError, Filter, identity::Identity};

/// Model B: outbound WebSocket relay client.
///
/// [`RelayClient::run`] loops forever: connect, `hello` / `hello_ok`
/// handshake, then a `query` → `result` loop, reconnecting after any session
/// error or clean close.
pub struct RelayClient<F: Filter> {
    identity: Arc<Identity>,
    filter: Arc<Mutex<F>>,
    url: String,
    reconnect: Duration,
}

impl<F: Filter> RelayClient<F> {
    pub fn new(
        identity: Arc<Identity>,
        filter: Arc<Mutex<F>>,
        url: impl Into<String>,
        reconnect: Duration,
    ) -> Self {
        Self {
            identity,
            filter,
            url: url.into(),
            reconnect,
        }
    }

    /// Run the relay loop forever, reconnecting after any session error.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.session().await {
                tracing::warn!(
                    agent = %self.identity.name,
                    url   = %self.url,
                    error = %e,
                    "relay session error, reconnecting"
                );
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }

    /// Open one relay session: handshake -> query/result loop -> return on close/error.
    async fn session(&self) -> Result<(), EmFilterError> {
        tracing::info!(agent = %self.identity.name, url = %self.url, "connecting to relay");

        let (ws, _) = connect_async(&self.url).await?;
        let (mut write, mut read) = ws.split();

        write
            .send(Message::Text(self.identity.hello_payload().to_string().into()))
            .await?;

        let ack = read
            .next()
            .await
            .ok_or_else(|| EmFilterError::Protocol("relay closed before hello_ok".into()))??;
        let ack_text = ack
            .to_text()
            .map_err(|_| EmFilterError::Protocol("hello_ok frame was not text".into()))?;
        let ack_v: Value = serde_json::from_str(ack_text).unwrap_or(Value::Null);
        if ack_v["action"] != "hello_ok" {
            return Err(EmFilterError::Protocol(format!("hello rejected: {ack_v}")));
        }

        tracing::info!(agent = %self.identity.name, "relay hello_ok — entering query loop");

        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(text) => {
                    let v: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => {
                            tracing::warn!(agent = %self.identity.name, "invalid JSON from relay, ignoring");
                            continue;
                        }
                    };

                    if v["action"] != "query" {
                        continue;
                    }

                    let Some(id) = v["id"].as_str().map(str::to_string) else {
                        tracing::warn!(agent = %self.identity.name, "query frame missing 'id', skipping");
                        continue;
                    };
                    let body = v["body"].as_str().unwrap_or("").trim().to_string();

                    tracing::info!(agent = %self.identity.name, query_id = %id, body = %body, "handling query");

                    let items = {
                        let mut f = self.filter.lock().await;
                        match f.handle(&body).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(
                                    agent    = %self.identity.name,
                                    query_id = %id,
                                    error    = %e,
                                    "filter handler returned error"
                                );
                                json!([])
                            }
                        }
                    };

                    let (signer_id, signature) = self.identity.sign_results(&items);
                    write
                        .send(Message::Text(
                            json!({
                                "action": "result",
                                "id": id,
                                "results": items,
                                "signer_id": signer_id,
                                "signature": signature,
                            })
                            .to_string()
                            .into(),
                        ))
                        .await?;
                }
                Message::Close(_) => {
                    tracing::info!(agent = %self.identity.name, "relay closed the connection");
                    return Ok(());
                }
                _ => {
                    tracing::debug!(agent = %self.identity.name, "ignoring non-text WebSocket frame");
                }
            }
        }

        Ok(())
    }
}
