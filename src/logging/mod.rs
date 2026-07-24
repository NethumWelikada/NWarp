use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Phase 7: structured JSON logging. Each log line is a standalone
/// JSON object (line-delimited JSON / "JSONL"), which is directly
/// ingestible by log pipelines that speak OpenTelemetry's log data
/// model without a custom parser - e.g. the OpenTelemetry Collector's
/// `filelog` receiver, Vector, or Fluent Bit can all read this format
/// as-is and forward it into an OTel pipeline for correlation with
/// traces/metrics elsewhere in a stack.
///
/// To be precise about scope: this is NOT the OpenTelemetry SDK and
/// does not export via OTLP, and there is no trace/span context
/// propagation - it is structured logging in a format an OTel
/// collector can consume, not a full OTel instrumentation of the
/// server. Full OTLP export (via the `opentelemetry` crate) is a
/// reasonable follow-up, not implemented here.
#[derive(Serialize)]
struct AccessLogEntry<'a> {
    timestamp: u64,
    level: &'static str,
    event: &'static str,
    method: &'a str,
    path: &'a str,
    status: u16,
    peer: &'a str,
    server: &'static str,
}

#[derive(Serialize)]
struct ErrorLogEntry<'a> {
    timestamp: u64,
    level: &'static str,
    event: &'static str,
    message: &'a str,
    server: &'static str,
}

pub struct Logger {
    access_path: String,
    error_path: String,
    lock: Mutex<()>,
}

impl Logger {
    pub fn new(access_path: &str, error_path: &str) -> Logger {
        ensure_parent_dir(access_path);
        ensure_parent_dir(error_path);
        Logger {
            access_path: access_path.to_string(),
            error_path: error_path.to_string(),
            lock: Mutex::new(()),
        }
    }

    pub fn access(&self, method: &str, path: &str, status: u16, peer: &str) {
        let entry = AccessLogEntry {
            timestamp: timestamp(),
            level: "INFO",
            event: "http_request",
            method,
            path,
            status,
            peer,
            server: "NWarp",
        };
        self.write_json(&self.access_path, &entry);
    }

    pub fn error(&self, msg: &str) {
        let entry = ErrorLogEntry {
            timestamp: timestamp(),
            level: "ERROR",
            event: "error",
            message: msg,
            server: "NWarp",
        };
        self.write_json(&self.error_path, &entry);
    }

    fn write_json<T: Serialize>(&self, path: &str, entry: &T) {
        let line = match serde_json::to_string(entry) {
            Ok(mut s) => {
                s.push('\n');
                s
            }
            Err(_) => return,
        };

        let _guard = self.lock.lock().unwrap();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
        print!("{}", line);
    }
}

fn ensure_parent_dir(path: &str) {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
