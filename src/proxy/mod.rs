use crate::config::Config;
use crate::http::request::Request;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// A single upstream target within a proxy route. `healthy` is updated
/// in the background by the health-check thread (Phase 3.5) and read
/// on every request without locking.
pub struct UpstreamTarget {
    pub addr: String,
    healthy: AtomicBool,
}

/// A single proxy route: a path prefix mapped to one or more upstream
/// targets, load-balanced round-robin among the currently healthy ones.
pub struct ProxyRoute {
    pub prefix: String,
    targets: Vec<UpstreamTarget>,
    counter: AtomicUsize,
}

impl ProxyRoute {
    /// Picks the next healthy upstream in round-robin order. Tries at
    /// most one full lap around the target list; if every target is
    /// currently marked unhealthy, returns None so the caller can fail
    /// fast with 503 instead of hanging on a connection attempt to a
    /// known-dead host.
    fn pick_upstream(&self) -> Option<&str> {
        let n = self.targets.len();
        if n == 0 {
            return None;
        }
        for _ in 0..n {
            let idx = self.counter.fetch_add(1, Ordering::Relaxed) % n;
            let target = &self.targets[idx];
            if target.healthy.load(Ordering::Relaxed) {
                return Some(target.addr.as_str());
            }
        }
        None
    }
}

/// The full set of configured proxy routes, built once at startup from
/// `proxy_route` lines in nwarp.conf and shared (read-only structure,
/// atomic health/counter state) across all connection threads and the
/// background health-check thread.
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
                targets: upstreams
                    .iter()
                    .map(|addr| UpstreamTarget {
                        addr: addr.clone(),
                        // Assume healthy until the first check proves
                        // otherwise, so cold start doesn't block traffic.
                        healthy: AtomicBool::new(true),
                    })
                    .collect(),
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

    /// True if this table has at least one configured route (used to
    /// decide whether it's worth spawning the health-check thread).
    pub fn has_routes(&self) -> bool {
        !self.routes.is_empty()
    }
}

/// Spawns the Phase 3.5 background health-check thread. Every
/// `interval`, it attempts a TCP connect (bounded by `timeout`) to
/// every configured upstream and updates its healthy/unhealthy flag.
/// This is a basic connectivity check (TCP reachability), not an
/// application-level check - documented as a known scope boundary.
pub fn spawn_health_checker(
    table: Arc<ProxyTable>,
    interval: Duration,
    timeout: Duration,
) -> thread::JoinHandle<()> {
    thread::spawn(move || loop {
        for route in &table.routes {
            for target in &route.targets {
                let ok = tcp_connect_ok(strip_scheme(&target.addr), timeout);
                target.healthy.store(ok, Ordering::Relaxed);
            }
        }
        thread::sleep(interval);
    })
}

fn tcp_connect_ok(addr: &str, timeout: Duration) -> bool {
    match addr.to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(sock_addr) => TcpStream::connect_timeout(&sock_addr, timeout).is_ok(),
            None => false,
        },
        Err(_) => false,
    }
}

/// Strips the scheme from an upstream URL (`http://host:port` ->
/// `host:port`). Used as both the TCP connect address and the
/// forwarded Host header. HTTPS upstreams are not yet supported -
/// proxying to an upstream over TLS is a later phase.
fn strip_scheme(upstream: &str) -> &str {
    upstream
        .trim_start_matches("https://")
        .trim_start_matches("http://")
}

/// Forwards a request to a healthy upstream (chosen round-robin) and
/// relays the raw response bytes straight back to the client. Returns
/// the parsed upstream status code for logging purposes.
///
/// Returns an error if no healthy upstream is available for this
/// route, or if the connection/relay itself fails - see
/// `server::connection::handle_generic` for how each case maps to a
/// client-facing status code (503 vs 502 respectively).
pub fn relay<S: Write>(
    req: &Request,
    route: &ProxyRoute,
    client: &mut S,
    peer: &str,
) -> io::Result<u16> {
    let upstream_addr = route
        .pick_upstream()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no healthy upstream available"))?;
    let upstream_addr = strip_scheme(upstream_addr);

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
