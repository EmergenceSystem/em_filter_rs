/// All errors produced by the em_filter library.
#[derive(Debug, thiserror::Error)]
pub enum EmFilterError {
    /// WebSocket protocol or connection error.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    /// JSON encoding or decoding failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// No disco nodes were configured or could be resolved.
    ///
    /// Reserved for future use — not currently returned by the library.
    /// Automatic resolution always falls back to `localhost:8080`.
    #[error("No disco nodes configured or resolved")]
    NoNodes,

    /// An error during HTML processing.
    #[error("HTML error: {0}")]
    Html(String),

    /// I/O error (e.g. reading emergence.conf).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
