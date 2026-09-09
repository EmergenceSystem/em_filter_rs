# em_filter

[![Crates.io](https://img.shields.io/crates/v/em_filter.svg)](https://crates.io/crates/em_filter)
[![Docs.rs](https://docs.rs/em_filter/badge.svg)](https://docs.rs/em_filter)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Rust SDK for building [Emergence](https://github.com/emergencesystem) network agents.

`em_filter` lets any Rust process join the Emergence distributed discovery network
as a **filter agent** — a service that receives search queries from an `em_disco`
node, processes them (web search, DNS lookup, LLM call, database query, …), and
returns structured, ed25519-signed results.

This crate joins the **signed Emergence mesh**: every agent has an ed25519 identity,
and every result it returns is signed. It speaks the same protocol as the other
Emergence SDKs (Python, Go, …) — see [`em_filter_py`](https://github.com/EmergenceSystem/em_filter_py)
for the canonical reference implementation.

---

## How it works

```
                     Model B — relay (default, NAT-friendly)
 ┌───────────────┐   outbound WSS     ┌─────────────┐
 │ FilterRunner  │ ─────────────────► │  em_disco   │
 │ (your agent)  │ ◄──── query ─────  │             │
 │               │ ──── result ─────► │             │
 └───────────────┘   (signed)         └─────────────┘

                     Model A — direct
 ┌───────────────┐  POST /agent/query ┌─────────────┐
 │ AgentServer   │ ◄───────────────── │  em_disco   │
 │ (your agent)  │ ── signed result ► │             │
 │               │ ── POST /pop/gossip (self-payload, periodic) ►
 └───────────────┘                    └─────────────┘
```

1. `FilterRunner` loads (or creates) the agent's ed25519 identity and resolves disco nodes.
2. `EM_FILTER_MODE` selects the transport: `relay` (default) opens an outbound WebSocket to
   `wss://<disco>/ws/filter`; `direct` runs a local HTTP server and gossips it to disco seeds;
   `both` runs them concurrently.
3. On a query, the transport calls your `Filter::handle`, signs the result with the agent's
   ed25519 key, and sends it back — `{results, signer_id, signature}`.

---

## Installation

```toml
[dependencies]
em_filter  = "0.1"
serde_json = "1"
tokio      = { version = "1", features = ["full"] }
```

`async_trait` is re-exported by the crate — no need to add it separately.

---

## Quick start

```rust
use em_filter::{async_trait, AgentConfig, EmFilterError, Filter, FilterRunner};
use serde_json::{json, Value};

struct MyFilter;

#[async_trait]
impl Filter for MyFilter {
    async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
        // `body` is the raw query string, e.g. "erlang otp"
        Ok(json!([{
            "type": "url",
            "properties": {
                "url":   "https://example.com",
                "title": format!("Result for: {}", body)
            }
        }]))
    }

    fn capabilities(&self) -> Vec<String> {
        vec!["search".into(), "query".into(), "web".into()]
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
        .run()
        .await
        .unwrap();
}
```

By default the agent runs in `relay` mode against `localhost:8080`. Override via
environment variables or `AgentConfig` — see [Configuration](#configuration).

---

## Try the built-in example

The crate ships an `echo_filter` example — the fastest way to verify that your
disco node is reachable and the handshake works:

```bash
EM_FILTER_MODE=relay EM_DISCO_HOST=disco.roques.me cargo run --example echo_filter
```

Expected output once connected:

```
INFO em_filter: Starting em_filter agent agent="echo_filter" mode="relay" nodes=1
INFO em_filter: connecting to relay agent="echo_filter" url="wss://disco.roques.me:443/ws/filter"
INFO em_filter: relay hello_ok — entering query loop agent="echo_filter"
```

---

## Building your own filter

Copy the echo example as your starting point:

```
filters/
└── my_filter/
    ├── Cargo.toml
    └── src/
        └── main.rs
```

**`Cargo.toml`:**
```toml
[package]
name    = "my_filter"
version = "0.1.0"
edition = "2021"

[dependencies]
em_filter           = "0.1"
serde_json          = "1"
tokio               = { version = "1", features = ["full"] }
tracing-subscriber  = "0.3"
```

**`src/main.rs`:**
```rust
use em_filter::{async_trait, AgentConfig, EmFilterError, Filter, FilterRunner};
use serde_json::{json, Value};

struct MyFilter;

#[async_trait]
impl Filter for MyFilter {
    async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
        tracing::info!(query = %body, "handling query");
        // … your logic here …
        Ok(json!([]))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    FilterRunner::new("my_filter", MyFilter, AgentConfig::default())
        .run()
        .await
        .unwrap();
}
```

---

## The Filter trait

Implement `Filter` on any struct that holds your agent's state:

```rust
use em_filter::{async_trait, EmFilterError, Filter};
use serde_json::{json, Value};

struct DnsFilter {
    // state — HTTP client, cache, counters, etc.
    cache: std::collections::HashMap<String, Vec<String>>,
}

#[async_trait]
impl Filter for DnsFilter {
    async fn handle(&mut self, body: &str) -> Result<Value, EmFilterError> {
        // Resolve body as a domain name, return a DNS embryo.
        Ok(json!([{
            "type": "dns",
            "properties": { "domain": body, "ips": ["93.184.216.34"] }
        }]))
    }

    fn capabilities(&self) -> Vec<String> {
        vec!["search".into(), "query".into(), "dns".into(), "network".into()]
    }
}
```

### Result format

`handle` returns a `serde_json::Value` — typically a JSON array of **embryo** objects.
Each embryo has a `"type"` string and a `"properties"` map:

| Type | Required properties |
|------|---------------------|
| `"url"` | `url`, `title` |
| `"dns"` | `domain`, `ips` |
| `"text"` | `content` |

An empty array (`json!([])`) or `Value::Null` means "no results for this query".

### Capabilities

The `capabilities()` method returns the list of capabilities your agent advertises.
em_disco uses this to route queries — a query with `capabilities = ["dns"]` is
delivered only to agents that advertise `"dns"`.

Default: `["search", "query"]`. Override to add domain-specific capabilities.

---

## Configuration

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `EM_FILTER_MODE` | `relay` | `relay` \| `direct` \| `both` |
| `EM_DISCO_HOST` | — | Disco node hostname |
| `EM_DISCO_PORT` | — | Disco node port |
| `EM_FILTER_KEY_DIR` | `./empop_key_<name>/` | ed25519 key file directory |
| `EM_FILTER_RECONNECT_MS` | `5000` | Relay reconnect delay in milliseconds |
| `EM_FILTER_QUERY_PORT` | `9600` | Model A (`direct`) HTTP listen port |
| `EM_FILTER_ADVERTISE_HOST` | `0.0.0.0` | Model A host advertised in gossip payloads |
| `EM_FILTER_GOSSIP_INTERVAL_S` | `5` | Model A gossip push interval (seconds) |

### Node resolution order

1. `AgentConfig::disco_nodes` — explicit list (highest priority)
2. `EM_DISCO_HOST` / `EM_DISCO_PORT` env vars
3. `[em_disco] nodes = …` in `emergence.conf`
4. `localhost:8080` — built-in default

### TLS inference

| Host | Port | Transport |
|------|------|-----------|
| `localhost`, `127.0.0.1`, `::1` | any | `ws://` (plain) |
| any other | 443, or unspecified | `wss://` (TLS) |
| any other | other explicit port | `ws://` (plain) |

### emergence.conf

```ini
[em_disco]
nodes = localhost:8080, disco.example.com, [::1]:9000
```

Platform paths:
- **Linux / macOS:** `~/.config/emergence/emergence.conf`
- **Windows:** `%APPDATA%\emergence\emergence.conf`

### Programmatic configuration

```rust
use em_filter::{AgentConfig, DiscoNode};

let config = AgentConfig {
    jwt_token: None,
    disco_nodes: vec![
        DiscoNode { host: "disco.roques.me".into(), port: 443, tls: true },
    ],
};
```

---

## Multi-node and `both` mode

In `relay` mode, `FilterRunner` connects to the first resolved disco node. In
`direct` mode, the gossip pusher advertises the agent's identity to every
resolved node (`AgentConfig::disco_nodes`, or `EM_DISCO_HOST`/`EM_DISCO_PORT`,
or `emergence.conf`). `EM_FILTER_MODE=both` runs the relay WebSocket and the
direct HTTP server + gossip pusher concurrently, under the same identity:

```rust
use em_filter::{AgentConfig, DiscoNode, FilterRunner};

let config = AgentConfig {
    disco_nodes: vec![
        DiscoNode { host: "disco.roques.me".into(), port: 443, tls: true },
    ],
    ..AgentConfig::default()
};

// or set EM_FILTER_MODE=both in the environment
FilterRunner::new("my_filter", MyFilter, config)
    .with_mode("both")
    .run()
    .await
    .unwrap();
```

---

## HTML utilities

A set of helpers for processing web pages, useful in web-scraper filters:

```rust
use em_filter::{
    strip_scripts, get_text, extract_elements,
    extract_attribute, decode_html_entities, should_skip_link,
};

let html = r#"<p>Hello <b>world</b></p><script>alert(1)</script>"#;

// Remove <script> blocks
let clean = strip_scripts(html).unwrap();

// Extract plain text
let text = get_text(&clean); // "Hello world"

// CSS selector extraction
let links = extract_elements(html, "a.result");

// Attribute extraction
let href = extract_attribute(r#"<a href="/page">link</a>"#, "href");
// → Some("/page")

// Entity decoding
let decoded = decode_html_entities("caf&eacute; &amp; croissant");
// → "café & croissant"

// Skip ad / tracker links
let skip = should_skip_link("https://ads.example.com", &["ads.example.com"]);
// → true
```

---

## Wire protocol

Every result is signed with the agent's ed25519 key (see `em_filter::crypto`), so
the disco (and Emquest, downstream) can verify it came from the identity it
gossip-bound, without trusting the transport.

**Model B — relay (agent ↔ disco, WebSocket to `/ws/filter`):**
```json
{ "action": "hello",    "name": "<agent_name>", "pubkey": "<b64>", "sig": "<b64>", "capabilities": ["search", "query", "web"] }
{ "action": "hello_ok", "id": "<b64>" }
{ "action": "query",    "id": "<query_id>", "body": "<query_string>" }
{ "action": "result",   "id": "<query_id>", "results": [...], "signer_id": "<b64>", "signature": "<b64>" }
```

**Model A — direct (agent serves HTTP):**
```text
POST /agent/query   { "query": "<query_string>" }
                  -> { "results": [...], "signer_id": "<b64>", "signature": "<b64>" }
POST /pop/gossip     <remote self-payload>
                  -> { "id": "<b64>", "name": "...", "host": "...", "query_port": N,
                        "pubkey": "<b64>", "sig": "<b64>", "capabilities": [...], "role": "filter" }
GET  /health      -> "ok"
```

The library handles the handshake, signing, and reconnection automatically. Your
code only implements `Filter::handle`.

---

## Logging

The library uses [`tracing`](https://docs.rs/tracing). Add `tracing-subscriber`
to your binary crate to see connection logs:

```toml
[dependencies]
tracing-subscriber = "0.3"
```

```rust
tracing_subscriber::fmt::init();
```

Log levels:
- `INFO` — connection lifecycle (connecting, `hello_ok`, disconnected)
- `WARN` — connection errors, malformed frames, query ID issues
- `ERROR` — task panics

---

## License

[MIT](LICENSE)
