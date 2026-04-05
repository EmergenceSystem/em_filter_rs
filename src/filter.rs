use async_trait::async_trait;
use serde_json::Value;

use crate::EmFilterError;

/// The handler contract for an Emergence filter agent.
///
/// Implement this trait on a struct that holds your agent's state — HTTP clients,
/// caches, database connections, counters, etc. The library calls [`Filter::handle`]
/// for every `query` frame received from em_disco.
///
/// State lives in the struct fields rather than being passed in and out (unlike the
/// Erlang `em_filter` `handle(Body, Memory) -> {Result, NewMemory}` callback).
///
/// # Example
///
/// ```
/// use em_filter::{async_trait, EmFilterError, Filter};
/// use serde_json::{json, Value};
///
/// struct DnsFilter {
///     cache: std::collections::HashMap<String, String>,
/// }
///
/// #[async_trait]
/// impl Filter for DnsFilter {
///     async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
///         // Look up `body` as a domain name, return a DNS embryo.
///         Ok(json!([{
///             "type": "dns",
///             "properties": { "domain": body, "ips": ["93.184.216.34"] }
///         }]))
///     }
///
///     fn capabilities(&self) -> Vec<String> {
///         vec!["search".into(), "query".into(), "dns".into(), "network".into()]
///     }
/// }
/// ```
#[async_trait]
pub trait Filter: Send + 'static {
    /// Handle an incoming query from em_disco.
    ///
    /// `body` is the raw query string sent by the user (e.g. `"erlang otp"`).
    ///
    /// Return a JSON value — typically an array of *embryo* objects. Each embryo
    /// has a `"type"` string and a `"properties"` map. The most common types are:
    ///
    /// | Type | Required properties |
    /// |------|---------------------|
    /// | `"url"` | `url`, `title` |
    /// | `"dns"` | `domain`, `ips` |
    /// | `"text"` | `content` |
    ///
    /// Returning `Value::Null` or `json!([])` is valid and means "no results for
    /// this query". On error, return `Err(EmFilterError::Html(…))` or any
    /// [`EmFilterError`] variant — the connection logs the error, sends `null` as
    /// the result, and continues running.
    async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError>;

    /// Capabilities announced to em_disco in the `agent_hello` handshake frame.
    ///
    /// em_disco uses capability lists to route queries: a query sent with
    /// `capabilities = ["dns"]` is delivered only to agents that advertise `"dns"`.
    /// If no agent matches, em_disco falls back to broadcasting to all agents.
    ///
    /// Defaults to `["search", "query"]` — the base capabilities shared by all
    /// em_filter agents. Override to declare domain-specific capabilities such as
    /// `"web"`, `"dns"`, `"rss"`, or `"llm"`.
    fn capabilities(&self) -> Vec<String> {
        vec!["search".into(), "query".into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct NoopFilter;

    #[async_trait]
    impl Filter for NoopFilter {
        async fn handle(&mut self, _body: &str) -> Result<Value, EmFilterError> {
            Ok(json!([]))
        }
    }

    struct CustomCapsFilter;

    #[async_trait]
    impl Filter for CustomCapsFilter {
        async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
            Ok(json!({"echo": body}))
        }

        fn capabilities(&self) -> Vec<String> {
            vec!["search".into(), "query".into(), "web".into()]
        }
    }

    #[tokio::test]
    async fn test_handle_returns_value() {
        let mut f = NoopFilter;
        let result = f.handle("test query").await.unwrap();
        assert_eq!(result, json!([]));
    }

    #[test]
    fn test_default_capabilities() {
        let f = NoopFilter;
        let caps = f.capabilities();
        assert!(caps.contains(&"search".to_string()));
        assert!(caps.contains(&"query".to_string()));
        assert_eq!(caps.len(), 2);
    }

    #[test]
    fn test_custom_capabilities() {
        let f = CustomCapsFilter;
        let caps = f.capabilities();
        assert!(caps.contains(&"web".to_string()));
        assert_eq!(caps.len(), 3);
    }

    #[tokio::test]
    async fn test_handle_can_use_body() {
        let mut f = CustomCapsFilter;
        let result = f.handle("erlang").await.unwrap();
        assert_eq!(result["echo"], "erlang");
    }
}
