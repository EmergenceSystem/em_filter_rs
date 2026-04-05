//! # em_filter — Rust SDK for the Emergence network
//!
//! `em_filter` lets any Rust process join the [Emergence](https://github.com/emergencesystem)
//! distributed discovery network as a **filter agent**. A filter agent receives search queries
//! from the `em_disco` broker, processes them (web search, DNS lookup, LLM call, database query,
//! …), and returns structured results.
//!
//! This crate is the Rust equivalent of the Erlang `em_filter` library — same WebSocket
//! protocol, same configuration contract, idiomatic Rust API.
//!
//! ---
//!
//! ## How it works
//!
//! ```text
//!  ┌─────────────┐    WebSocket     ┌───────────────┐    WebSocket     ┌──────────────┐
//!  │  em_disco   │ ◄─────────────── │  FilterRunner  │ ─────────────── │  em_disco    │
//!  │  (broker)   │  query / result  │  (your agent)  │  (multi-node)   │  (replica)   │
//!  └─────────────┘                  └───────────────┘                  └──────────────┘
//!                                          │
//!                                   Arc<Mutex<F>>
//!                                          │
//!                                   ┌──────┴──────┐
//!                                   │ your Filter  │
//!                                   │    impl      │
//!                                   └─────────────┘
//! ```
//!
//! 1. [`FilterRunner`] resolves disco nodes from config / env / defaults.
//! 2. It spawns one tokio task per node; each task maintains a persistent WebSocket connection
//!    with automatic reconnection.
//! 3. When em_disco sends a `query` frame, the task acquires the shared `Arc<Mutex<F>>`,
//!    calls your [`Filter::handle`] implementation, and sends back a `result` frame.
//!
//! ---
//!
//! ## Quick start
//!
//! Add to `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! em_filter  = "0.1"
//! serde_json = "1"
//! tokio      = { version = "1", features = ["full"] }
//! ```
//!
//! Implement the [`Filter`] trait and run it:
//!
//! ```no_run
//! use em_filter::{async_trait, AgentConfig, EmFilterError, Filter, FilterRunner};
//! use serde_json::{json, Value};
//!
//! struct MyFilter;
//!
//! #[async_trait]
//! impl Filter for MyFilter {
//!     async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
//!         // `body` is the raw query string, e.g. "erlang otp"
//!         Ok(json!([{
//!             "type": "url",
//!             "properties": {
//!                 "url":   "https://example.com",
//!                 "title": format!("Result for: {}", body)
//!             }
//!         }]))
//!     }
//!
//!     fn capabilities(&self) -> Vec<String> {
//!         vec!["search".into(), "query".into(), "web".into()]
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     tracing_subscriber::fmt::init();
//!     FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
//!         .run()
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! By default the agent connects to `localhost:8080`. See [`AgentConfig`] for
//! environment variables and `emergence.conf` configuration.
//!
//! A runnable `echo_filter` example is included in the crate. It connects to
//! em_disco and echoes every query back — useful for verifying your broker setup:
//!
//! ```bash
//! cargo run --example echo_filter
//! ```
//!
//! ---
//!
//! ## Configuration
//!
//! Node discovery follows this priority order (same as the Erlang library):
//!
//! | Priority | Source |
//! |----------|--------|
//! | 1 | `AgentConfig::disco_nodes` (explicit) |
//! | 2 | `EM_DISCO_HOST` / `EM_DISCO_PORT` env vars |
//! | 3 | `[em_disco] nodes = …` in `emergence.conf` |
//! | 4 | `localhost:8080` (built-in default) |
//!
//! **Environment variables:**
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `EM_DISCO_HOST` | — | Disco broker hostname |
//! | `EM_DISCO_PORT` | — | Disco broker port |
//! | `EM_FILTER_JWT_TOKEN` | — | JWT for authenticated brokers |
//! | `EM_FILTER_RECONNECT_MS` | `5000` | Reconnect delay in milliseconds |
//!
//! **TLS is inferred automatically:**
//! - `localhost` / `127.0.0.1` / `::1` → plain WebSocket (`ws://`)
//! - Remote host on port 443 → TLS WebSocket (`wss://`)
//! - Remote host on any other port → plain WebSocket (`ws://`)
//!
//! ---
//!
//! ## HTML utilities
//!
//! The crate ships a small set of HTML helpers useful when scraping web pages.
//! They mirror the Erlang `em_filter` module's function signatures:
//!
//! ```no_run
//! use em_filter::{strip_scripts, get_text, extract_elements, extract_attribute,
//!                 decode_html_entities, should_skip_link};
//!
//! let html = r#"<p>Hello <b>world</b></p><script>alert(1)</script>"#;
//!
//! let clean   = strip_scripts(html).unwrap();           // removes <script> blocks
//! let text    = get_text(&clean);                        // "Hello world"
//! let links   = extract_elements(html, "a");             // Vec<String> of inner HTML
//! let href    = extract_attribute(r#"<a href="/x">"#, "href"); // Some("/x")
//! let decoded = decode_html_entities("caf&eacute;");     // "café"
//! let skip    = should_skip_link("https://ads.com", &["ads.com"]); // true
//! ```
//!
//! ---
//!
//! ## WebSocket protocol
//!
//! The agent speaks a simple JSON-over-WebSocket protocol to em_disco:
//!
//! **Agent → Disco:**
//! ```json
//! { "action": "register",    "name": "<agent_name>" }
//! { "action": "agent_hello", "capabilities": ["search", "query", "web"] }
//! { "action": "result",      "id": "<query_id>", "data": <result> }
//! ```
//!
//! **Disco → Agent:**
//! ```json
//! { "status": "ok", "action": "registered" }
//! { "status": "ok", "action": "agent_registered", "capabilities": [...] }
//! { "action": "query", "id": "<query_id>", "body": "<query_string>" }
//! ```

// Re-export async_trait so users don't need to add it as a direct dependency.
pub use async_trait::async_trait;

mod error;
mod filter;
mod config;
mod html;
mod connection;
mod runner;

pub use error::EmFilterError;
pub use filter::Filter;
pub use config::{AgentConfig, DiscoNode};
pub use html::{
    strip_scripts, get_text, extract_elements, extract_attribute,
    decode_html_entities, should_skip_link,
};
pub use runner::FilterRunner;
