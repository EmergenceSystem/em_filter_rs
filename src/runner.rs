use std::sync::Arc;
use tokio::sync::Mutex;
use crate::{Filter, AgentConfig, EmFilterError};
use crate::connection::Connection;

/// Runs a filter agent by connecting to all configured em_disco nodes.
///
/// Each node gets its own tokio task with an independent WebSocket connection.
/// All tasks share the same filter instance via `Arc<Mutex<F>>`, so handler
/// calls are serialized — one query is processed at a time regardless of how
/// many nodes are connected. This is safe because filters are typically I/O-bound.
///
/// `run` does not return unless all connection tasks panic (which should not happen
/// in normal operation).
///
/// # Example
///
/// ```no_run
/// use em_filter::{Filter, FilterRunner, AgentConfig, async_trait};
/// use em_filter::EmFilterError;
/// use serde_json::Value;
///
/// struct MyFilter;
///
/// #[async_trait]
/// impl Filter for MyFilter {
///     async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
///         Ok(serde_json::json!([]))
///     }
/// }
///
/// #[tokio::main]
/// async fn main() {
///     FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
///         .run()
///         .await
///         .unwrap();
/// }
/// ```
pub struct FilterRunner<F: Filter> {
    /// Agent name announced in the `register` frame.
    name: String,
    /// Shared filter instance — wrapped in Arc<Mutex> so all node tasks can use it.
    filter: Arc<Mutex<F>>,
    /// Agent configuration (JWT token, disco nodes).
    config: AgentConfig,
}

impl<F: Filter> FilterRunner<F> {
    /// Create a new runner.
    ///
    /// `name` is the agent name announced during the em_disco handshake.
    /// It must match the `sub` claim of the JWT if authentication is enabled.
    pub fn new(name: impl Into<String>, filter: F, config: AgentConfig) -> Self {
        Self {
            name: name.into(),
            filter: Arc::new(Mutex::new(filter)),
            config,
        }
    }

    /// Start the agent. Spawns one tokio task per disco node and runs indefinitely.
    ///
    /// Each task reconnects automatically on connection loss — identical lifecycle
    /// to the Erlang `em_filter_server` gen_server.
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
            let _ = handle.await;
        }

        Ok(())
    }
}
