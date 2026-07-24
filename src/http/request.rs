use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub version: String,
    pub headers: HashMap<String, String>,
}

impl Request {
    /// Parses an HTTP/1.x request line + headers off the socket.
    /// Body parsing is intentionally left out of Phase 1 (static file
    /// serving only needs the request line + headers).
    pub fn parse(stream: &TcpStream) -> std::io::Result<Request> {
        let mut reader = BufReader::new(stream.try_clone()?);

        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;
        let request_line = request_line.trim_end();

        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let raw_path = parts.next().unwrap_or("/").to_string();
        let version = parts.next().unwrap_or("HTTP/1.1").to_string();

        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = line.trim_end().split_once(':') {
                headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }

        Ok(Request {
            method,
            path: sanitize_path(&raw_path),
            version,
            headers,
        })
    }
}

/// Strips query strings and blocks directory traversal attempts
/// (`../`) before the path is ever used for filesystem lookups.
fn sanitize_path(raw: &str) -> String {
    let path = raw.split('?').next().unwrap_or("/");
    let decoded = percent_decode(path);
    if decoded.contains("..") {
        return "/".to_string();
    }
    decoded
}

/// Minimal percent-decoder for URL paths (%20 -> space, etc).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[allow(dead_code)]
pub fn read_exact_body(stream: &TcpStream, len: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut s = stream.try_clone()?;
    s.read_exact(&mut buf)?;
    Ok(buf)
}
