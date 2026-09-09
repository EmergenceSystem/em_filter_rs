//! # echo_filter — minimal working example
//!
//! Joins the Emergence mesh as an agent named `echo_filter`, announcing the
//! `["search", "query", "echo"]` capabilities, and echoes every query it
//! receives back as a single URL embryo.
//!
//! Use this as a starting point when building your own filter, or to verify
//! that your disco node is reachable and the handshake works.
//!
//! ## Run (Model B — relay, default)
//!
//! ```bash
//! EM_FILTER_MODE=relay EM_DISCO_HOST=disco.roques.me cargo run --example echo_filter
//! ```
//!
//! ## Run (Model A — direct)
//!
//! ```bash
//! EM_FILTER_MODE=direct EM_DISCO_HOST=disco.roques.me EM_FILTER_QUERY_PORT=9600 \
//! cargo run --example echo_filter
//! ```
//!
//! ## Environment variables
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `EM_FILTER_MODE` | `relay` | `relay` \| `direct` \| `both` |
//! | `EM_DISCO_HOST` | `localhost` | Disco node hostname |
//! | `EM_DISCO_PORT` | — | Disco node port (defaults to 443/TLS for a remote host) |
//! | `EM_FILTER_KEY_DIR` | `./empop_key_echo_filter/` | ed25519 key file directory |
//! | `EM_FILTER_RECONNECT_MS` | `5000` | Relay reconnect delay (ms) |
//! | `EM_FILTER_QUERY_PORT` | `9600` | Model A HTTP listen port |

use em_filter::{async_trait, AgentConfig, EmFilterError, Filter, FilterRunner};
use serde_json::{json, Value};

struct EchoFilter;

#[async_trait]
impl Filter for EchoFilter {
    async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
        tracing::info!(query = %body, "Received query");
        Ok(json!([{
            "type": "url",
            "properties": {
                "url":   "https://example.com",
                "title": format!("Echo: {}", body)
            }
        }]))
    }

    fn capabilities(&self) -> Vec<String> {
        vec!["search".into(), "query".into(), "echo".into()]
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    FilterRunner::new("echo_filter", EchoFilter, AgentConfig::default())
        .run()
        .await
        .unwrap();
}
