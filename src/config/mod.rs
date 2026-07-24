use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Runtime configuration for NWarp, loaded from an Apache/Nginx-style
/// key = value config file (see configs/default.conf).
#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub document_root: String,
    pub server_name: String,
    pub index_file: String,
    pub worker_threads: usize,
    pub access_log: String,
    pub error_log: String,
    pub tls_enabled: bool,
    pub tls_port: u16,
    pub tls_cert: String,
    pub tls_key: String,
    /// (path_prefix, upstream_urls) pairs, parsed from `proxy_route`
    /// lines. Empty by default - proxying is fully opt-in and does not
    /// affect static file serving unless configured.
    pub proxy_routes: Vec<(String, Vec<String>)>,
    /// How often (seconds) to actively health-check each configured
    /// upstream. Phase 3.5.
    pub health_check_interval_secs: u64,
    /// Per-check connect timeout (seconds).
    pub health_check_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: "0.0.0.0".to_string(),
            port: 9090,
            document_root: "./www".to_string(),
            server_name: "NWarp".to_string(),
            index_file: "index.html".to_string(),
            worker_threads: 4,
            access_log: "./logs/access.log".to_string(),
            error_log: "./logs/error.log".to_string(),
            tls_enabled: false,
            tls_port: 9443,
            tls_cert: "./certs/dev-cert.pem".to_string(),
            tls_key: "./certs/dev-key.pem".to_string(),
            proxy_routes: Vec::new(),
            health_check_interval_secs: 5,
            health_check_timeout_secs: 2,
        }
    }
}

impl Config {
    /// Load config from a file path. Falls back to defaults for any
    /// key not present, and to full defaults if the file is missing.
    pub fn load(path: &str) -> Config {
        let mut cfg = Config::default();

        if !Path::new(path).exists() {
            eprintln!(
                "[nwarp] warning: config file '{}' not found, using built-in defaults",
                path
            );
            return cfg;
        }

        let contents = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[nwarp] warning: could not read '{}': {}", path, e);
                return cfg;
            }
        };

        let map = parse_kv(&contents);

        if let Some(v) = map.get("host") {
            cfg.host = v.clone();
        }
        if let Some(v) = map.get("port") {
            if let Ok(p) = v.parse::<u16>() {
                cfg.port = p;
            }
        }
        if let Some(v) = map.get("document_root") {
            cfg.document_root = v.clone();
        }
        if let Some(v) = map.get("server_name") {
            cfg.server_name = v.clone();
        }
        if let Some(v) = map.get("index") {
            cfg.index_file = v.clone();
        }
        if let Some(v) = map.get("worker_threads") {
            if let Ok(n) = v.parse::<usize>() {
                cfg.worker_threads = n.max(1);
            }
        }
        if let Some(v) = map.get("access_log") {
            cfg.access_log = v.clone();
        }
        if let Some(v) = map.get("error_log") {
            cfg.error_log = v.clone();
        }
        if let Some(v) = map.get("tls_enabled") {
            cfg.tls_enabled = v.eq_ignore_ascii_case("true") || v == "1";
        }
        if let Some(v) = map.get("tls_port") {
            if let Ok(p) = v.parse::<u16>() {
                cfg.tls_port = p;
            }
        }
        if let Some(v) = map.get("tls_cert") {
            cfg.tls_cert = v.clone();
        }
        if let Some(v) = map.get("tls_key") {
            cfg.tls_key = v.clone();
        }

        if let Some(v) = map.get("health_check_interval") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.health_check_interval_secs = n.max(1);
            }
        }
        if let Some(v) = map.get("health_check_timeout") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.health_check_timeout_secs = n.max(1);
            }
        }

        // proxy_route <prefix> = <upstream1>,<upstream2>,...
        // Each line has a unique key ("proxy_route /api", "proxy_route /app"),
        // so the flat key=value map from parse_kv already separates them.
        for (k, v) in &map {
            if let Some(prefix) = k.strip_prefix("proxy_route ") {
                let upstreams: Vec<String> = v
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !upstreams.is_empty() {
                    cfg.proxy_routes.push((prefix.trim().to_string(), upstreams));
                }
            }
        }

        cfg
    }
}

/// Parses simple `key = value` lines. Lines starting with `#` are comments.
fn parse_kv(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(
                k.trim().to_string(),
                v.trim().trim_matches('"').to_string(),
            );
        }
    }
    map
}
