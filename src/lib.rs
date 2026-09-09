//! # em_filter — Rust SDK for the Emergence signed mesh
//!
//! `em_filter` lets any Rust process join the [Emergence](https://github.com/emergencesystem)
//! distributed discovery network as a **filter agent**. A filter agent receives search queries
//! from an `em_disco` node, processes them (web search, DNS lookup, LLM call, database query,
//! …), and returns structured, **ed25519-signed** results.
//!
//! ---
//!
//! ## How it works
//!
//! Every agent has an ed25519 identity ([`crypto`], [`Identity`]) persisted to a key file on
//! first run. [`FilterRunner`] loads that identity and starts one or both mesh transports,
//! selected by `EM_FILTER_MODE`:
//!
//! ```text
//!                          Model B (relay, default)
//!  ┌──────────────┐   outbound WSS    ┌─────────────┐
//!  │ FilterRunner │ ────────────────► │   em_disco  │
//!  │ (your agent) │ ◄──── query ───── │             │
//!  │              │ ──── result ────► │             │
//!  └──────────────┘   (signed)        └─────────────┘
//!
//!                          Model A (direct)
//!  ┌──────────────┐  POST /agent/query ┌─────────────┐
//!  │ AgentServer  │ ◄───────────────── │   em_disco  │
//!  │ (your agent) │ ── signed result ► │             │
//!  │              │ ── POST /pop/gossip (self-payload, periodic) ►
//!  └──────────────┘                    └─────────────┘
//! ```
//!
//! - **Model B — relay (default, NAT-friendly):** [`FilterRunner`] opens an outbound WebSocket
//!   to `wss://<disco>/ws/filter`, sends a signed `hello`, and answers `query` frames with
//!   signed `result` frames. No inbound reachability needed.
//! - **Model A — direct:** [`FilterRunner`] runs a small HTTP server (`/agent/query`,
//!   `/pop/gossip`, `/health`) and periodically gossips its own signed identity to each
//!   configured disco seed so it can be queried directly.
//! - **`both`:** runs Model A and Model B concurrently under the same identity.
//!
//! In every case, the shared `Arc<Mutex<F>>` around your [`Filter`] impl serializes handler
//! calls — one query is processed at a time, mirroring the single-process model of the Erlang
//! `em_filter` library.
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
//!     // EM_FILTER_MODE=relay EM_DISCO_HOST=disco.roques.me cargo run
//!     FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
//!         .run()
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! A runnable `echo_filter` example is included in the crate:
//!
//! ```bash
//! EM_FILTER_MODE=relay EM_DISCO_HOST=disco.roques.me cargo run --example echo_filter
//! ```
//!
//! ---
//!
//! ## Configuration
//!
//! **Disco node resolution** (used to build the relay URL / gossip seeds), same priority order
//! as the other Emergence SDKs:
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
//! | `EM_FILTER_MODE` | `relay` | `relay` \| `direct` \| `both` |
//! | `EM_DISCO_HOST` | — | Disco node hostname |
//! | `EM_DISCO_PORT` | — | Disco node port |
//! | `EM_FILTER_KEY_DIR` | `./empop_key_<name>/` | ed25519 key file directory |
//! | `EM_FILTER_RECONNECT_MS` | `5000` | Relay reconnect delay (ms) |
//! | `EM_FILTER_QUERY_PORT` | `9600` | Model A HTTP listen port |
//! | `EM_FILTER_ADVERTISE_HOST` | `0.0.0.0` | Model A host advertised in gossip |
//! | `EM_FILTER_GOSSIP_INTERVAL_S` | `5` | Model A gossip push interval (seconds) |
//!
//! **TLS is inferred automatically:**
//! - `localhost` / `127.0.0.1` / `::1` → plain WebSocket (`ws://`)
//! - Remote host on port 443 → TLS WebSocket (`wss://`)
//! - Remote host on any other port → plain WebSocket (`ws://`)
//! - A remote host with no explicit port defaults to `443` (TLS).
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
//! ## Wire protocol
//!
//! **Model B — relay (filter ↔ disco, WebSocket):**
//! ```json
//! { "action": "hello",    "name": "...", "pubkey": "<b64>", "sig": "<b64>", "capabilities": [...] }
//! { "action": "hello_ok", "id": "<b64>" }
//! { "action": "query",    "id": "<qid>", "body": "<query_string>" }
//! { "action": "result",   "id": "<qid>", "results": [...], "signer_id": "<b64>", "signature": "<b64>" }
//! ```
//!
//! **Model A — direct (filter serves HTTP):**
//! ```text
//! POST /agent/query  { "query": "<query_string>" }
//!                  -> { "results": [...], "signer_id": "<b64>", "signature": "<b64>" }
//! POST /pop/gossip   <remote self-payload>
//!                  -> <our own self-payload>
//! GET  /health     -> "ok"
//! ```
//!
//! All signatures are ed25519 over the canonical byte forms in [`crypto`] — see that module for
//! the exact `canonical_identity` / `canonical_response` layouts, which are byte-identical to
//! the Erlang reference implementation.

// Re-export async_trait so users don't need to add it as a direct dependency.
pub use async_trait::async_trait;

mod config;
mod error;
mod filter;
mod html;
mod identity;
mod runner;
mod server;
mod wsclient;
pub mod crypto;

pub use config::{AgentConfig, DiscoNode};
pub use error::EmFilterError;
pub use filter::Filter;
pub use html::{
    decode_html_entities, extract_attribute, extract_elements, get_text, should_skip_link,
    strip_scripts,
};
pub use identity::Identity;
pub use runner::FilterRunner;
pub use server::{AgentServer, GossipPusher, GossipPusherHandle};
pub use wsclient::RelayClient;
