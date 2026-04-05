/// All errors produced by the em_filter library.
///
/// Most operations in this crate are infallible at the library level — connection
/// errors are logged and trigger a reconnect rather than propagating. The variants
/// below surface only in contexts where the caller must handle them:
///
/// - [`EmFilterError::WebSocket`] / [`EmFilterError::Json`] — returned by the
///   internal connection machinery; exposed so custom [`crate::Filter`] impls can
///   construct or match them.
/// - [`EmFilterError::Html`] — returned by [`crate::strip_scripts`] and available
///   for use in [`crate::Filter::handle`] implementations that process HTML.
/// - [`EmFilterError::Io`] — may appear if `emergence.conf` cannot be read,
///   though the library treats this silently and falls back to defaults.
#[derive(Debug, thiserror::Error)]
pub enum EmFilterError {
    /// WebSocket protocol or transport error.
    ///
    /// Wraps [`tokio_tungstenite::tungstenite::Error`] and is produced by the
    /// connection layer when a frame cannot be sent or received.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    /// JSON encoding or decoding failure.
    ///
    /// Can be returned from [`crate::Filter::handle`] if your implementation
    /// serialises data and encounters an encoding error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// No disco nodes were configured or could be resolved.
    ///
    /// Reserved for future use — not currently returned by the library.
    /// The built-in fallback chain always resolves to `localhost:8080` when no
    /// other configuration is present.
    #[error("No disco nodes configured or resolved")]
    NoNodes,

    /// An error during HTML processing.
    ///
    /// Returned by [`crate::strip_scripts`] when regex compilation fails (never
    /// in practice — the pattern is a compile-time constant). Available for use
    /// in [`crate::Filter::handle`] implementations that process HTML.
    #[error("HTML error: {0}")]
    Html(String),

    /// I/O error, for example when `emergence.conf` cannot be read.
    ///
    /// The library handles this silently in node resolution. Exposed here so
    /// filter implementations can produce I/O errors from `handle`.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
