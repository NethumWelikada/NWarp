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
