//! Mode dispatcher for an em_filter agent — mirrors the Python reference,
//! `em_filter/runner.py`'s `FilterRunner`.
//!
//! `EM_FILTER_MODE` (default `relay`) selects the transport:
//! - `relay`  — Model B: outbound WS to `wss://<disco>/ws/filter` (NAT-friendly, default).
//! - `direct` — Model A: local HTTP server (`/agent/query`, `/pop/gossip`,
//!   `/health`) plus a gossip push loop advertising it to each disco seed.
//! - `both`   — runs the HTTP server + gossip pusher *and* the relay WS
//!   concurrently, under the same identity.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::identity::Identity;
use crate::server::{AgentServer, GossipPusher};
use crate::wsclient::RelayClient;
use crate::{AgentConfig, EmFilterError, Filter};

/// Runs a filter agent under one or both mesh transports, selected by
/// `EM_FILTER_MODE`.
///
/// # Example — default (relay) mode
///
/// ```no_run
/// use em_filter::{async_trait, AgentConfig, EmFilterError, Filter, FilterRunner};
/// use serde_json::{json, Value};
///
/// struct MyFilter;
///
/// #[async_trait]
/// impl Filter for MyFilter {
///     async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
///         Ok(json!([{
///             "type": "url",
///             "properties": { "url": "https://example.com", "title": body }
///         }]))
///     }
/// }
///
/// #[tokio::main]
/// async fn main() {
///     tracing_subscriber::fmt::init();
///     // EM_FILTER_MODE=relay EM_DISCO_HOST=disco.roques.me
///     FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
///         .run()
///         .await
///         .unwrap();
/// }
/// ```
pub struct FilterRunner<F: Filter> {
    name: String,
    filter: Arc<Mutex<F>>,
    config: AgentConfig,
    mode: Option<String>,
}

impl<F: Filter> FilterRunner<F> {
    /// Create a new runner. `name` is the agent name used in the identity's
    /// self-signature, the `hello` frame, and the gossip payload.
    pub fn new(name: impl Into<String>, filter: F, config: AgentConfig) -> Self {
        Self {
            name: name.into(),
            filter: Arc::new(Mutex::new(filter)),
            config,
            mode: None,
        }
    }

    /// Override the transport mode instead of reading `EM_FILTER_MODE`.
    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// Start the configured transport(s) and run forever.
    ///
    /// Reads `EM_FILTER_MODE` (default `relay`), `EM_DISCO_HOST` /
    /// `EM_DISCO_PORT` (via [`AgentConfig::resolve_nodes`]), `EM_FILTER_KEY_DIR`
    /// (default `./empop_key_<name>/`), and — for `direct`/`both` —
    /// `EM_FILTER_QUERY_PORT` (default `9600`), `EM_FILTER_ADVERTISE_HOST`
    /// (default `0.0.0.0`), and `EM_FILTER_GOSSIP_INTERVAL_S` (default `5`).
    pub async fn run(self) -> Result<(), EmFilterError> {
        let mode = self
            .mode
            .clone()
            .unwrap_or_else(|| std::env::var("EM_FILTER_MODE").unwrap_or_else(|_| "relay".into()));

        if !matches!(mode.as_str(), "direct" | "relay" | "both") {
            return Err(EmFilterError::Protocol(format!(
                "unknown EM_FILTER_MODE: {mode:?}"
            )));
        }

        let nodes = self.config.resolve_nodes()?;

        let key_dir = std::env::var("EM_FILTER_KEY_DIR")
            .unwrap_or_else(|_| format!("./empop_key_{}/", self.name));
        let capabilities = self.filter.lock().await.capabilities();
        let identity = Arc::new(Identity::new(self.name.clone(), &key_dir, capabilities)?);

        tracing::info!(
            agent = %self.name,
            mode  = %mode,
            nodes = nodes.len(),
            "Starting em_filter agent"
        );

        let mut server_thread = None;
        let mut pusher_handle = None;

        if mode == "direct" || mode == "both" {
            let query_port: u16 = std::env::var("EM_FILTER_QUERY_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(9600);
            let advertise_host =
                std::env::var("EM_FILTER_ADVERTISE_HOST").unwrap_or_else(|_| "0.0.0.0".into());
            let gossip_interval_s: f64 = std::env::var("EM_FILTER_GOSSIP_INTERVAL_S")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5.0);

            let server = AgentServer::bind(
                Arc::clone(&identity),
                Arc::clone(&self.filter),
                "0.0.0.0",
                query_port,
                advertise_host.clone(),
            )?;
            let bound_port = server.port;
            server_thread = Some(server.start());

            let seeds: Vec<(String, u16)> =
                nodes.iter().map(|n| (n.host.clone(), n.port)).collect();
            let pusher = GossipPusher::new(
                Arc::clone(&identity),
                seeds,
                advertise_host,
                bound_port,
                Duration::from_secs_f64(gossip_interval_s),
            );
            pusher_handle = Some(pusher.start());
        }

        if mode == "relay" || mode == "both" {
            let node = nodes.first().cloned();
            if let Some(node) = node {
                let scheme = if node.tls { "wss" } else { "ws" };
                let url = format!("{scheme}://{}:{}/ws/filter", node.host, node.port);
                let relay = RelayClient::new(
                    Arc::clone(&identity),
                    Arc::clone(&self.filter),
                    url,
                    reconnect_delay(),
                );
                relay.run().await; // blocks forever
            }
        } else {
            // direct-only: server/pusher run on their own OS threads; block here.
            if let Some(h) = server_thread {
                let _ = h.join();
            }
            if let Some(h) = pusher_handle {
                h.stop();
            }
        }

        Ok(())
    }
}

/// Returns the relay reconnect delay, sampled once per session.
///
/// Reads `EM_FILTER_RECONNECT_MS` from the environment; defaults to 5000 ms.
fn reconnect_delay() -> Duration {
    let ms: u64 = std::env::var("EM_FILTER_RECONNECT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    Duration::from_millis(ms)
}
