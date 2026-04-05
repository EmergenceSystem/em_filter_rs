//! Rust SDK for building Emergence network agents.
//!
//! # Quick start
//!
//! ```no_run
//! use em_filter::{Filter, FilterRunner, AgentConfig};
//! use em_filter::async_trait;
//! use serde_json::Value;
//!
//! struct MyFilter;
//!
//! #[em_filter::async_trait]
//! impl Filter for MyFilter {
//!     async fn handle(&mut self, body: &str) -> Value {
//!         serde_json::json!([{"type": "url", "properties": {"url": "https://example.com"}}])
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
//!         .run()
//!         .await
//!         .unwrap();
//! }
//! ```

// Re-export async_trait so users don't need to add it as a direct dependency.
pub use async_trait::async_trait;

mod error;
mod filter;

pub use error::EmFilterError;
pub use filter::Filter;

// These modules will be added in subsequent tasks.
// Declare them here so lib.rs is the single source of public API.
// (Commented out until implemented)
// mod config;
// mod html;
// mod connection;
// mod runner;
// pub use config::{AgentConfig, DiscoNode};
// pub use html::{strip_scripts, get_text, extract_elements, extract_attribute, decode_html_entities, should_skip_link};
// pub use runner::FilterRunner;
