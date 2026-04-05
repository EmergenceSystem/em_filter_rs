use std::sync::Arc;
use tokio::sync::Mutex;
use crate::{Filter, AgentConfig, EmFilterError};
use crate::connection::Connection;

/// Runs a filter agent by connecting to all configured em_disco nodes.
///
/// `FilterRunner` is the main entry point for the library. It:
///
/// 1. Resolves disco nodes from [`AgentConfig`] (explicit list, env vars,
///    `emergence.conf`, or the built-in default `localhost:8080`).
/// 2. Wraps the [`Filter`] implementation in `Arc<Mutex<F>>` so it can be
///    shared safely across tasks.
/// 3. Spawns one [`tokio::task`] per node; each task maintains a persistent
///    WebSocket connection and reconnects automatically on any error.
///
/// All handler calls are serialized through the single `Arc<Mutex<F>>` — one
/// query is processed at a time regardless of how many nodes are connected.
/// This mirrors the single-process model of the Erlang em_filter library and
/// is safe for the typical I/O-bound filter.
///
/// `run` does not return in normal operation. It only returns after all
/// connection tasks have exited (which should not happen — each task loops
/// forever, reconnecting on errors).
///
/// # Example — minimal agent
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
///     FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
///         .run()
///         .await
///         .unwrap();
/// }
/// ```
///
/// # Example — connecting to a specific node with a JWT
///
/// ```no_run
/// use em_filter::{AgentConfig, DiscoNode, Filter, FilterRunner, async_trait, EmFilterError};
/// use serde_json::Value;
///
/// struct MyFilter;
/// # #[async_trait] impl Filter for MyFilter {
/// #     async fn handle(&mut self, _: &str) -> Result<Value, EmFilterError> { Ok(serde_json::json!([])) }
/// # }
///
/// #[tokio::main]
/// async fn main() {
///     let config = AgentConfig {
///         jwt_token: Some("eyJ...".into()),
///         disco_nodes: vec![
///             DiscoNode { host: "disco.example.com".into(), port: 443, tls: true },
///         ],
///     };
///     FilterRunner::new("my_filter", MyFilter, config)
///         .run()
///         .await
///         .unwrap();
/// }
/// ```
pub struct FilterRunner<F: Filter> {
    /// Agent name announced in the `register` frame.
    name: String,
    /// Shared filter instance — wrapped in `Arc<Mutex<F>>` so all node tasks can use it.
    filter: Arc<Mutex<F>>,
    /// Agent configuration (JWT token, disco nodes).
    config: AgentConfig,
}

impl<F: Filter> FilterRunner<F> {
    /// Create a new runner.
    ///
    /// `name` is the agent name sent in the `register` handshake frame. It must
    /// match the `sub` claim of the JWT token when em_disco authentication is enabled.
    pub fn new(name: impl Into<String>, filter: F, config: AgentConfig) -> Self {
        Self {
            name: name.into(),
            filter: Arc::new(Mutex::new(filter)),
            config,
        }
    }

    /// Start the agent and run indefinitely.
    ///
    /// Spawns one tokio task per resolved disco node. Each task runs the
    /// connect → register → agent_hello → message loop → reconnect lifecycle
    /// identical to the Erlang `em_filter_server` gen_server.
    ///
    /// Returns `Ok(())` only after all tasks have exited (unexpected in normal
    /// operation). Panics in individual tasks are logged and do not propagate.
    pub async fn run(self) -> Result<(), EmFilterError> {
        let nodes = self.config.resolve_nodes()?;
        let jwt_token = self.config.resolve_jwt();

        tracing::info!(
            agent = %self.name,
            nodes = nodes.len(),
            "Starting em_filter agent"
        );

        let mut handles = Vec::with_capacity(nodes.len());

        for node in nodes {
            let conn = Connection::new(
                self.name.clone(),
                node,
                Arc::clone(&self.filter),
                jwt_token.clone(),
            );
            handles.push(tokio::spawn(async move {
                conn.run().await;
            }));
        }

        // Wait for all tasks. In normal operation these run forever.
        for handle in handles {
            if let Err(e) = handle.await {
                tracing::error!(agent = %self.name, error = ?e, "Connection task panicked");
            }
        }

        Ok(())
    }
}
