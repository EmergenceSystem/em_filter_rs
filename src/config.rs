/// A single em_disco node to connect to.
#[derive(Debug, Clone)]
pub struct DiscoNode {
    /// Hostname or IP address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Use TLS (wss://) when true, plain TCP (ws://) when false.
    pub tls: bool,
}

/// Configuration for a filter agent.
///
/// All fields are optional. If `disco_nodes` is empty, nodes are resolved
/// automatically in the same priority order as the Erlang em_filter library:
///
/// 1. `disco_nodes` in this struct (if non-empty)
/// 2. `EM_DISCO_HOST` / `EM_DISCO_PORT` environment variables
/// 3. `[em_disco] nodes = ...` in `emergence.conf`
/// 4. Default: `localhost:8080` TCP
#[derive(Debug, Clone, Default)]
pub struct AgentConfig {
    /// JWT token passed as `?token=<jwt>` on WebSocket upgrade.
    ///
    /// If `None`, the `EM_FILTER_JWT_TOKEN` environment variable is checked.
    pub jwt_token: Option<String>,

    /// Explicit list of disco nodes.
    ///
    /// If empty, nodes are resolved automatically.
    pub disco_nodes: Vec<DiscoNode>,
}

impl AgentConfig {
    /// Resolve the list of disco nodes to connect to.
    ///
    /// Returns `Err(EmFilterError::NoNodes)` only when an explicit empty list
    /// is provided via `disco_nodes` — never for the automatic resolution path
    /// (which always falls back to `localhost:8080`).
    pub(crate) fn resolve_nodes(&self) -> Result<Vec<DiscoNode>, crate::EmFilterError> {
        if !self.disco_nodes.is_empty() {
            return Ok(self.disco_nodes.clone());
        }
        // Priority 2: environment variables
        let host_env = std::env::var("EM_DISCO_HOST").ok();
        let port_env = std::env::var("EM_DISCO_PORT").ok();
        match (host_env, port_env) {
            (Some(host), Some(port_str)) => {
                let port: u16 = port_str.parse().unwrap_or(8080);
                let tls = infer_tls(&host, port);
                return Ok(vec![DiscoNode { host, port, tls }]);
            }
            (Some(host), None) => {
                let (port, tls) = default_port_tls(&host);
                return Ok(vec![DiscoNode { host, port, tls }]);
            }
            (None, Some(port_str)) => {
                let port: u16 = port_str.parse().unwrap_or(8080);
                return Ok(vec![DiscoNode {
                    host: "localhost".into(),
                    port,
                    tls: false,
                }]);
            }
            (None, None) => {}
        }
        // Priority 3: emergence.conf
        if let Some(nodes) = read_conf_nodes() {
            if !nodes.is_empty() {
                return Ok(nodes);
            }
        }
        // Priority 4: default
        Ok(vec![DiscoNode {
            host: "localhost".into(),
            port: 8080,
            tls: false,
        }])
    }

    /// Resolve the JWT token.
    ///
    /// Returns the token from this struct if set, otherwise checks the
    /// `EM_FILTER_JWT_TOKEN` environment variable.
    pub(crate) fn resolve_jwt(&self) -> Option<String> {
        self.jwt_token
            .clone()
            .or_else(|| std::env::var("EM_FILTER_JWT_TOKEN").ok())
    }
}

/// Returns true for TLS based on host and port.
///
/// localhost / 127.0.0.1 → always TCP.
/// Remote host on port 443 → TLS.
/// Remote host on any other port → TCP.
fn infer_tls(host: &str, port: u16) -> bool {
    if host == "localhost" || host == "127.0.0.1" {
        return false;
    }
    port == 443
}

/// Returns the default port and TLS flag for a host with no explicit port.
///
/// localhost / 127.0.0.1 → (8080, false).
/// Any other host → (443, true).
fn default_port_tls(host: &str) -> (u16, bool) {
    if host == "localhost" || host == "127.0.0.1" {
        (8080, false)
    } else {
        (443, true)
    }
}

/// Read and parse disco nodes from emergence.conf.
fn read_conf_nodes() -> Option<Vec<DiscoNode>> {
    let path = conf_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    parse_conf(&content)
}

/// Platform-specific path to emergence.conf.
///
/// Unix:    `~/.config/emergence/emergence.conf`
/// Windows: `%APPDATA%/emergence/emergence.conf`
#[cfg(windows)]
fn conf_path() -> Option<std::path::PathBuf> {
    std::env::var("APPDATA").ok().map(|appdata| {
        std::path::PathBuf::from(appdata)
            .join("emergence")
            .join("emergence.conf")
    })
}

/// Platform-specific path to emergence.conf.
///
/// Unix:    `~/.config/emergence/emergence.conf`
/// Windows: `%APPDATA%/emergence/emergence.conf`
#[cfg(not(windows))]
fn conf_path() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(|home| {
        std::path::PathBuf::from(home)
            .join(".config")
            .join("emergence")
            .join("emergence.conf")
    })
}

/// Parse the `[em_disco] nodes = ...` entry from an INI-style config file.
fn parse_conf(content: &str) -> Option<Vec<DiscoNode>> {
    let mut section = String::new();
    let mut nodes_str: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with(';') || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        if section == "em_disco" {
            if let Some((key, val)) = line.split_once('=') {
                if key.trim() == "nodes" {
                    nodes_str = Some(val.trim().to_string());
                }
            }
        }
    }

    nodes_str.map(|s| parse_nodes(&s))
}

/// Parse a comma-separated node list: `localhost:8080, example.com`.
fn parse_nodes(s: &str) -> Vec<DiscoNode> {
    s.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            if let Some((host, port_str)) = entry.rsplit_once(':') {
                let host = host.trim().to_string();
                let port: u16 = port_str.trim().parse().ok()?;
                let tls = infer_tls(&host, port);
                Some(DiscoNode { host, port, tls })
            } else {
                let host = entry.to_string();
                let (port, tls) = default_port_tls(&host);
                Some(DiscoNode { host, port, tls })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_resolves_to_localhost() {
        std::env::remove_var("EM_DISCO_HOST");
        std::env::remove_var("EM_DISCO_PORT");
        let config = AgentConfig::default();
        let nodes = config.resolve_nodes().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].host, "localhost");
        assert_eq!(nodes[0].port, 8080);
        assert!(!nodes[0].tls);
    }

    #[test]
    fn test_explicit_nodes_override_env() {
        let config = AgentConfig {
            disco_nodes: vec![DiscoNode {
                host: "myhost.com".into(),
                port: 9000,
                tls: false,
            }],
            jwt_token: None,
        };
        let nodes = config.resolve_nodes().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].host, "myhost.com");
        assert_eq!(nodes[0].port, 9000);
    }

    #[test]
    fn test_localhost_is_always_tcp() {
        assert!(!infer_tls("localhost", 443));
        assert!(!infer_tls("127.0.0.1", 443));
        assert!(!infer_tls("localhost", 8080));
    }

    #[test]
    fn test_remote_port_443_is_tls() {
        assert!(infer_tls("example.com", 443));
    }

    #[test]
    fn test_remote_other_port_is_tcp() {
        assert!(!infer_tls("example.com", 8080));
    }

    #[test]
    fn test_default_port_tls_localhost() {
        let (port, tls) = default_port_tls("localhost");
        assert_eq!(port, 8080);
        assert!(!tls);
    }

    #[test]
    fn test_default_port_tls_remote() {
        let (port, tls) = default_port_tls("example.com");
        assert_eq!(port, 443);
        assert!(tls);
    }

    #[test]
    fn test_parse_conf_single_node_with_port() {
        let content = "[em_disco]\nnodes = localhost:8080\n";
        let nodes = parse_conf(content).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].host, "localhost");
        assert_eq!(nodes[0].port, 8080);
        assert!(!nodes[0].tls);
    }

    #[test]
    fn test_parse_conf_two_nodes() {
        let content = "[em_disco]\nnodes = localhost:8080, disco.example.com\n";
        let nodes = parse_conf(content).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].host, "localhost");
        assert_eq!(nodes[0].port, 8080);
        assert_eq!(nodes[1].host, "disco.example.com");
        assert_eq!(nodes[1].port, 443);
        assert!(nodes[1].tls);
    }

    #[test]
    fn test_parse_conf_ignores_comments() {
        let content = "; this is a comment\n[em_disco]\n# another comment\nnodes = localhost:9000\n";
        let nodes = parse_conf(content).unwrap();
        assert_eq!(nodes[0].port, 9000);
    }

    #[test]
    fn test_resolve_jwt_from_struct() {
        let config = AgentConfig {
            jwt_token: Some("my-token".into()),
            disco_nodes: vec![],
        };
        assert_eq!(config.resolve_jwt(), Some("my-token".to_string()));
    }

    #[test]
    fn test_resolve_jwt_none_when_absent() {
        std::env::remove_var("EM_FILTER_JWT_TOKEN");
        let config = AgentConfig::default();
        assert_eq!(config.resolve_jwt(), None);
    }
}
