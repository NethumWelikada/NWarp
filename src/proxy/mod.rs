use crate::config::Config;
use crate::http::request::Request;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A single proxy route: a path prefix mapped to one or more upstream
/// servers, load-balanced round-robin.
pub struct ProxyRoute {
    pub prefix: String,
    pub upstreams: Vec<String>,
    counter: AtomicUsize,
}

impl ProxyRoute {
    /// Picks the next upstream in round-robin order.
    fn pick_upstream(&self) -> &str {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.upstreams.len();
        &self.upstreams[idx]
    }
}

/// The full set of configured proxy routes, built once at startup from
/// `proxy_route` lines in nwarp.conf and shared (read-only, aside from
/// the atomic round-robin counters) across all connection threads.
pub struct ProxyTable {
    routes: Vec<ProxyRoute>,
}

impl ProxyTable {
    pub fn from_config(cfg: &Config) -> ProxyTable {
        let routes = cfg
            .proxy_routes
            .iter()
            .map(|(prefix, upstreams)| ProxyRoute {
                prefix: prefix.clone(),
                upstreams: upstreams.clone(),
                counter: AtomicUsize::new(0),
            })
            .collect();
        ProxyTable { routes }
    }

    /// Finds the longest matching prefix for the given request path.
    /// Returns None if no proxy route matches, meaning the request
    /// should fall through to static file serving as usual.
    pub fn match_route(&self, path: &str) -> Option<&ProxyRoute> {
        self.routes
            .iter()
            .filter(|r| path.starts_with(r.prefix.as_str()))
            .max_by_key(|r| r.prefix.len())
    }
}

/// Strips the scheme from an upstream URL (`http://host:port` ->
/// `host:port`). Used as both the TCP connect address and the
/// forwarded Host header. HTTPS upstreams are not yet supported in
/// Phase 3 - proxying to an upstream over TLS is a later phase.
fn strip_scheme(upstream: &str) -> &str {
    upstream
        .trim_start_matches("https://")
        .trim_start_matches("http://")
}

/// Forwards a request to the given upstream and relays the raw
/// response bytes straight back to the client. Returns the parsed
/// upstream status code for logging purposes.
pub fn relay<S: Write>(
    req: &Request,
    route: &ProxyRoute,
    client: &mut S,
    peer: &str,
) -> std::io::Result<u16> {
    let upstream_addr = strip_scheme(route.pick_upstream());

    let mut upstream = TcpStream::connect(upstream_addr)?;
    upstream.set_read_timeout(Some(Duration::from_secs(10)))?;
    upstream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let mut request_text = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nX-Forwarded-For: {}\r\nConnection: close\r\n",
        req.method, req.path, upstream_addr, peer
    );
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("host") || k.eq_ignore_ascii_case("connection") {
            continue;
        }
        request_text.push_str(&format!("{}: {}\r\n", k, v));
    }
    request_text.push_str("\r\n");

    upstream.write_all(request_text.as_bytes())?;

    let mut response_bytes = Vec::new();
    upstream.read_to_end(&mut response_bytes)?;

    let status = extract_status(&response_bytes);

    client.write_all(&response_bytes)?;
    client.flush()?;

    Ok(status)
}

fn extract_status(response: &[u8]) -> u16 {
    let text = String::from_utf8_lossy(response);
    text.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(502)
}
