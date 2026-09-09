//! Model A (direct): a small HTTP server exposing signed `/agent/query`,
//! `/pop/gossip` and `/health`, plus a gossip push loop advertising this
//! agent to each configured disco seed. Mirrors `em_pop`'s agent-facing HTTP
//! API — see the Python reference, `em_filter/server.py`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};
use tiny_http::{Method, Response, Server};
use tokio::sync::Mutex;

use crate::{Filter, identity::Identity};

/// Model A: direct HTTP server exposing `/agent/query`, `/pop/gossip` and
/// `/health`.
///
/// Each request that must call the (async) [`Filter::handle`] is served by
/// blocking on a dedicated single-threaded Tokio runtime owned by this
/// server, so [`AgentServer::run`] / [`AgentServer::start`] must be called
/// from a plain OS thread — not from inside an existing Tokio runtime.
pub struct AgentServer<F: Filter> {
    identity: Arc<Identity>,
    filter: Arc<Mutex<F>>,
    server: Server,
    /// Host advertised in the `/pop/gossip` self-payload (not necessarily the bind host).
    pub advertise_host: String,
    /// Bound TCP port (resolves a requested port of `0` to the OS-assigned one).
    pub port: u16,
}

impl<F: Filter> AgentServer<F> {
    /// Bind the HTTP listener on `host:port` (port `0` picks a free port).
    pub fn bind(
        identity: Arc<Identity>,
        filter: Arc<Mutex<F>>,
        host: &str,
        port: u16,
        advertise_host: impl Into<String>,
    ) -> std::io::Result<Self> {
        let addr = format!("{host}:{port}");
        let server = Server::http(&addr)
            .map_err(|e| std::io::Error::other(format!("failed to bind {addr}: {e}")))?;
        let bound_port = match server.server_addr() {
            tiny_http::ListenAddr::IP(a) => a.port(),
            #[allow(unreachable_patterns)]
            _ => port,
        };
        Ok(Self {
            identity,
            filter,
            server,
            advertise_host: advertise_host.into(),
            port: bound_port,
        })
    }

    /// Serve requests forever on the current thread.
    pub fn run(self) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build AgentServer runtime");

        for mut request in self.server.incoming_requests() {
            let method = request.method().clone();
            let url = request.url().to_string();

            match (&method, url.as_str()) {
                (Method::Get, "/health") => {
                    let _ = request.respond(Response::from_string("ok"));
                }
                (Method::Post, "/agent/query") => {
                    let mut body = String::new();
                    let _ = request.as_reader().read_to_string(&mut body);

                    let query = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| v.get("query").and_then(|q| q.as_str()).map(str::to_string));

                    let Some(query) = query else {
                        let resp = Response::from_string(json!({"error": "bad query"}).to_string())
                            .with_status_code(400);
                        let _ = request.respond(resp);
                        continue;
                    };

                    let filter = Arc::clone(&self.filter);
                    let outcome = rt.block_on(async move {
                        let mut f = filter.lock().await;
                        f.handle(&query).await
                    });

                    match outcome {
                        Ok(items) => {
                            let (signer_id, signature) = self.identity.sign_results(&items);
                            let body = json!({
                                "results": items,
                                "signer_id": signer_id,
                                "signature": signature,
                            });
                            let _ = request.respond(Response::from_string(body.to_string()));
                        }
                        Err(e) => {
                            let body = json!({"error": e.to_string()});
                            let resp = Response::from_string(body.to_string()).with_status_code(500);
                            let _ = request.respond(resp);
                        }
                    }
                }
                (Method::Post, "/pop/gossip") => {
                    // Minimal: accept the remote payload without processing it,
                    // reply with our own self-payload (matches server.py).
                    let mut buf = String::new();
                    let _ = request.as_reader().read_to_string(&mut buf);

                    let payload = self.identity.gossip_payload(&self.advertise_host, self.port);
                    let _ = request.respond(Response::from_string(payload.to_string()));
                }
                _ => {
                    let _ = request.respond(Response::from_string("").with_status_code(404));
                }
            }
        }
    }

    /// Spawn [`AgentServer::run`] on a dedicated OS thread.
    pub fn start(self) -> JoinHandle<()> {
        std::thread::spawn(move || self.run())
    }
}

/// Model A: periodically POSTs this identity's gossip payload to each seed
/// disco node's `/pop/gossip`, so it is discoverable for direct queries.
pub struct GossipPusher {
    identity: Arc<Identity>,
    seeds: Vec<(String, u16)>,
    host: String,
    query_port: u16,
    interval: Duration,
    stop: Arc<AtomicBool>,
}

impl GossipPusher {
    pub fn new(
        identity: Arc<Identity>,
        seeds: Vec<(String, u16)>,
        host: impl Into<String>,
        query_port: u16,
        interval: Duration,
    ) -> Self {
        Self {
            identity,
            seeds,
            host: host.into(),
            query_port,
            interval,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn push_once(&self) {
        let payload = self
            .identity
            .gossip_payload(&self.host, self.query_port)
            .to_string();
        for (host, port) in &self.seeds {
            if let Err(e) = http_post_json(host, *port, "/pop/gossip", &payload) {
                tracing::warn!(seed = %format!("{host}:{port}"), error = %e, "gossip push failed");
            }
        }
    }

    /// Spawn the push loop on a dedicated OS thread. Returns a handle that
    /// also stops the loop when it is dropped is *not* implied — call
    /// [`GossipPusherHandle::stop`] explicitly to end it.
    pub fn start(self) -> GossipPusherHandle {
        let stop = Arc::clone(&self.stop);
        let handle = std::thread::spawn(move || {
            while !self.stop.load(Ordering::Relaxed) {
                self.push_once();
                std::thread::sleep(self.interval);
            }
        });
        GossipPusherHandle { stop, handle }
    }
}

/// Handle to a running [`GossipPusher`] loop.
pub struct GossipPusherHandle {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl GossipPusherHandle {
    /// Signal the push loop to stop and wait for it to exit.
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

/// A minimal hand-rolled HTTP/1.1 POST — no external HTTP client dependency,
/// matching the Python reference (`urllib.request`), which likewise performs
/// a plain (non-TLS) POST regardless of the target node's own TLS flag.
fn http_post_json(host: &str, port: u16, path: &str, body: &str) -> std::io::Result<()> {
    let mut stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n\
         {body}",
        path = path,
        host = host,
        len = body.len(),
        body = body,
    );
    stream.write_all(request.as_bytes())?;

    // Drain the response so the peer sees a clean close; we don't need the body.
    let mut buf = [0u8; 512];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
