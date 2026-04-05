use async_trait::async_trait;
use serde_json::Value;

use crate::EmFilterError;

/// The handler contract for an Emergence agent.
///
/// Implement this trait on a struct that holds your agent's state
/// (HTTP clients, caches, counters, etc.). The library calls [`Filter::handle`]
/// for every query received from em_disco.
#[async_trait]
pub trait Filter: Send + 'static {
    /// Handle an incoming query from em_disco.
    ///
    /// `body` is the raw query string (e.g. `"erlang otp"`).
    /// Return a JSON value — typically an array of embryo objects:
    /// `[{"type": "url", "properties": {"url": "...", "title": "..."}}]`
    ///
    /// Returning `Value::Null` or an empty array is valid (no results).
    async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError>;

    /// Capabilities announced to em_disco via `agent_hello`.
    ///
    /// Defaults to `["search", "query"]` — the base capabilities shared
    /// by all em_filter agents. Override to add domain-specific caps
    /// such as `"web"`, `"dns"`, `"rss"`, or `"llm"`.
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
