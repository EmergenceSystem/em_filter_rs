# em_filter

Rust SDK for building [Emergence](https://github.com/emergencesystem) network agents.

`em_filter` lets any Rust process join the Emergence distributed discovery network
as a **filter agent** — a service that receives search queries from the `em_disco`
broker, processes them (web search, DNS lookup, LLM call, database query, …), and
returns structured results.

This crate is the Rust equivalent of the Erlang `em_filter` library: same WebSocket
protocol, same configuration contract, idiomatic Rust API.

---

## How it works

```
 ┌─────────────┐    WebSocket     ┌───────────────┐    WebSocket     ┌─────────────┐
 │  em_disco   │ ◄─────────────── │ FilterRunner  │ ───────────────  │  em_disco   │
 │  (broker)   │  query / result  │ (your agent)  │  (multi-node)    │  (replica)  │
 └─────────────┘                  └───────────────┘                  └─────────────┘
                                         │
                                  Arc<Mutex<F>>
                                         │
                                  ┌──────┴──────┐
                                  │ your Filter │
                                  │    impl     │
                                  └─────────────┘
```

1. `FilterRunner` resolves disco nodes and spawns one tokio task per node.
2. Each task maintains a persistent WebSocket connection with automatic reconnection.
3. On a `query` frame, the task calls your `Filter::handle` and sends back a `result` frame.

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

By default the agent connects to `localhost:8080`. Override via environment
variables or `AgentConfig` — see [Configuration](#configuration).

---

## Try the built-in example

The crate ships an `echo_filter` example — the fastest way to verify that your
em_disco broker is reachable and the handshake works:

```bash
# Clone / enter the crate directory, then:
cargo run --example echo_filter
```

Expected output once connected:

```
INFO em_filter: Starting em_filter agent agent="echo_filter" nodes=1
INFO em_filter: Connecting to em_disco agent="echo_filter" url="ws://localhost:8080/ws"
INFO em_filter: Registered on em_disco — entering message loop agent="echo_filter"
```

With a custom broker:

```bash
EM_DISCO_HOST=disco.example.com \
EM_DISCO_PORT=443 \
EM_FILTER_JWT_TOKEN=eyJ... \
cargo run --example echo_filter
```

Test it from the Erlang shell (with em_disco running):

```erlang
em_disco:query(<<"hello world">>).
%% → [#{<<"type">> => <<"url">>,
%%     <<"properties">> => #{<<"title">> => <<"Echo: hello world">>, ...}}]
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
| `EM_DISCO_HOST` | — | Disco broker hostname |
| `EM_DISCO_PORT` | — | Disco broker port |
| `EM_FILTER_JWT_TOKEN` | — | JWT for authenticated brokers |
| `EM_FILTER_RECONNECT_MS` | `5000` | Reconnect delay in milliseconds |

### Node resolution order

1. `AgentConfig::disco_nodes` — explicit list (highest priority)
2. `EM_DISCO_HOST` / `EM_DISCO_PORT` env vars
3. `[em_disco] nodes = …` in `emergence.conf`
4. `localhost:8080` — built-in default

### TLS inference

| Host | Port | Transport |
|------|------|-----------|
| `localhost`, `127.0.0.1`, `::1` | any | `ws://` (plain) |
| any other | 443 | `wss://` (TLS) |
| any other | other | `ws://` (plain) |

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
    jwt_token: Some("eyJ...".into()),
    disco_nodes: vec![
        DiscoNode { host: "disco.example.com".into(), port: 443, tls: true },
        DiscoNode { host: "disco2.example.com".into(), port: 443, tls: true },
    ],
};
```

---

## Multi-node

`FilterRunner` connects to all resolved nodes simultaneously. Each node gets its
own tokio task; the filter is shared via `Arc<Mutex<F>>`. All handler calls are
serialized — one query at a time — regardless of how many nodes are connected.

```rust
let config = AgentConfig {
    disco_nodes: vec![
        DiscoNode { host: "disco-eu.example.com".into(), port: 443, tls: true },
        DiscoNode { host: "disco-us.example.com".into(), port: 443, tls: true },
    ],
    ..AgentConfig::default()
};

FilterRunner::new("my_filter", MyFilter, config)
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

## WebSocket protocol

The agent speaks a minimal JSON-over-WebSocket protocol to em_disco.

**Agent → Disco:**
```json
{ "action": "register",    "name": "<agent_name>" }
{ "action": "agent_hello", "capabilities": ["search", "query", "web"] }
{ "action": "result",      "id": "<query_id>", "data": <result> }
```

**Disco → Agent:**
```json
{ "status": "ok", "action": "registered" }
{ "status": "ok", "action": "agent_registered", "capabilities": [...] }
{ "action": "query", "id": "<query_id>", "body": "<query_string>" }
```

The library handles the handshake and reconnection automatically. Your code only
implements `Filter::handle`.

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
- `INFO` — connection lifecycle (connected, disconnected, registered)
- `WARN` — connection errors, malformed frames, query ID issues
- `ERROR` — task panics

---

## License

MIT
