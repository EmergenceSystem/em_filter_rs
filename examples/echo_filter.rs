//! # echo_filter — minimal working example
//!
//! Connects to em_disco as an agent named `echo_filter`, announces the
//! `["search", "query", "echo"]` capabilities, and echoes every query it
//! receives back as a single URL embryo.
//!
//! Use this as a starting point when building your own filter, or to verify
//! that your em_disco broker is reachable and the handshake works.
//!
//! ## Run
//!
//! ```bash
//! cargo run --example echo_filter
//! ```
//!
//! With a custom broker address:
//!
//! ```bash
//! EM_DISCO_HOST=disco.example.com EM_DISCO_PORT=443 \
//! EM_FILTER_JWT_TOKEN=eyJ... \
//! cargo run --example echo_filter
//! ```
//!
//! ## Test it from the Erlang shell
//!
//! Once the agent is connected you should see:
//! ```text
//! INFO echo_filter: Registered on em_disco — entering message loop
//! ```
//!
//! Then from `em_disco`:
//! ```erlang
//! em_disco:query(<<"hello world">>).
//! %% → [#{<<"type">> => <<"url">>,
//! %%     <<"properties">> => #{<<"title">> => <<"Echo: hello world">>, ...}}]
//! ```
//!
//! The agent logs each query it receives:
//! ```text
//! INFO echo_filter: Received query query="hello world"
//! ```
//!
//! ## Environment variables
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `EM_DISCO_HOST` | `localhost` | Broker hostname |
//! | `EM_DISCO_PORT` | `8080` | Broker port |
//! | `EM_FILTER_JWT_TOKEN` | — | JWT for authenticated brokers |
//! | `EM_FILTER_RECONNECT_MS` | `5000` | Reconnect delay (ms) |

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
