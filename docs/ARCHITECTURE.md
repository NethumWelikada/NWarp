# NWarp Architecture

*A project by Nethum Welikada, Master of Engineering in Internetworking,
Dalhousie University, Halifax, Nova Scotia, Canada.*

## Phase 1: Foundation (done)

- **Concurrency model (original):** thread-per-connection, bounded by a
  fixed-size thread pool. Superseded by the async event loop in
  Phase 4 below - kept here for the historical record of how the
  project started.
- **Serving:** static files only, resolved under `document_root`.
  Directory traversal (`..`) is blocked at two layers: string sanitation
  in `http::request::sanitize_path`, and a canonicalized-path check in
  `http::router::path_is_within`.
- **Config:** flat `key = value` file, parsed in `config/mod.rs`.
- **Logging:** flat-file access/error logs, one line per request.

## Phase 2: TLS / HTTPS (done)

- **Library:** [rustls](https://github.com/rustls/rustls) 0.21 - a
  memory-safe TLS implementation in pure Rust (no OpenSSL C bindings,
  unlike Apache/Nginx's default TLS stack).
- **Protocol:** negotiates TLSv1.3 with modern AEAD cipher suites
  (verified via live handshake: `TLS_AES_256_GCM_SHA384` / X25519).
- **Design:** the HTTPS accept loop (`src/tls/mod.rs`) runs on its own
  thread with its own thread pool, alongside the existing plain-HTTP
  listener - both are active at once on separate ports
  (`port` / `tls_port`), matching how Apache/Nginx run HTTP and HTTPS
  virtual hosts side by side.
- **Code reuse:** `http::request::Request::parse` and
  `http::response::Response::send` were made generic over any
  `Read`/`Write` stream, so the exact same routing and logging path
  (`server::connection::handle_generic`) serves both a plain
  `TcpStream` and a TLS-wrapped `rustls::Stream` - no duplicated
  request-handling logic between HTTP and HTTPS.
- **Certs:** loaded from PEM files (`tls_cert` / `tls_key` in config),
  supporting both PKCS#8 and PKCS#1 (RSA) private key formats. A dev
  self-signed cert generator ships at `scripts/generate-dev-cert.sh`;
  production deployments should use a real CA (e.g. Let's Encrypt).

## Phase 3: Reverse proxy + load balancing (done)

- **Config:** `proxy_route <prefix> = <upstream1>,<upstream2>,...`
  lines in `nwarp.conf`, parsed into `Config::proxy_routes`. Fully
  opt-in - an empty list (the default) means every request falls
  through to static file serving exactly as in Phase 1.
- **Matching:** longest-prefix match across all configured routes
  (`proxy::ProxyTable::match_route`), checked before static file
  resolution in `connection::handle_generic`.
- **Load balancing:** simple round-robin via a per-route
  `AtomicUsize` counter (`proxy::ProxyRoute::pick_upstream`), safe to
  share across the thread pool without locking.
- **Relay:** `proxy::relay` opens a fresh TCP connection to the chosen
  upstream per request, rebuilds the request line and headers
  (forwarding `X-Forwarded-For`, overriding `Host`), and streams the
  raw upstream response back to the client byte-for-byte - verified
  end-to-end against two real upstream HTTP servers with alternating
  A/B/A/B/A/B round-robin.
- **Known limitations (intentionally not hidden):**
  - Upstreams must be plain HTTP; proxying to a TLS-terminated
    upstream is a later phase.
  - Request bodies are not forwarded yet (consistent with Phase 1's
    header-only request parsing).

## Phase 3.5: Upstream health checks (done)

- **Model:** each upstream target carries an `AtomicBool` healthy flag,
  read (lock-free) on every request and written by a single background
  thread (`proxy::spawn_health_checker`).
- **Check:** every `health_check_interval_secs`, attempt a TCP connect
  (bounded by `health_check_timeout_secs`) to each configured
  upstream; store the result. This is a connectivity check, not an
  application-level check (see limitations below).
- **Selection:** `ProxyRoute::pick_upstream` now does round-robin
  *among currently healthy targets only* - it tries up to one full lap
  around the target list looking for a healthy one, and returns `None`
  if every target in the route is unhealthy.
- **Client-facing behavior:** `None` from `pick_upstream` maps to an
  immediate `503 Service Unavailable` (fail fast, no wasted connection
  attempt to a dead host); an upstream that's marked healthy but fails
  mid-request still returns `502 Bad Gateway` as before.
- **Verified end-to-end:** killed one of two upstreams mid-test - after
  one health-check cycle, all subsequent requests correctly routed
  only to the surviving upstream; killing both returned `503` as
  designed (not `502`).
- **Known limitations (intentionally not hidden):**
  - TCP-level reachability only, not an HTTP-level check (e.g. hitting
    a `/health` endpoint and checking for `200`). A reachable-but-broken
    application would still be marked healthy.
  - Cold start assumes every upstream is healthy until the first check
    completes, so the very first `health_check_interval_secs` window
    could route to a dead upstream once before the first check catches
    it.

## Phase 4: Async event loop (done)

- **Runtime:** [Tokio](https://tokio.rs) multi-threaded scheduler,
  using epoll as its I/O reactor on Linux. Replaces the Phase 1
  thread-per-connection model (`server/pool.rs`, now removed) with
  Tokio tasks - lightweight, non-blocking units of work multiplexed
  onto a small, fixed pool of OS threads (`worker_threads` in config
  now sizes the Tokio runtime directly, via
  `tokio::runtime::Builder::new_multi_thread().worker_threads(n)`).
- **Why this matters:** under thread-per-connection, every open
  connection holds an entire OS thread (with its own stack, scheduling
  overhead, and a hard ceiling around what the OS can context-switch
  between efficiently) for as long as it's open, even while idle or
  waiting on I/O. Under the Tokio model, a connection's task yields
  control at every `.await` point (waiting on a socket read/write,
  a file read, a timer) instead of blocking a whole thread, so a
  handful of OS threads can service many thousands of concurrent
  connections - the same model Nginx's worker processes use
  internally (Nginx just implements its own event loop in C rather
  than using a runtime like Tokio).
- **What changed to make this work:** `http::request::Request::parse`
  and `http::response::Response::send` are now `async fn`, generic
  over `AsyncRead`/`AsyncWrite` (via `tokio::io`) instead of the
  blocking `std::io::Read`/`Write` traits used in Phase 1-3.5. Static
  file reads in `http::router::route` now use `tokio::fs` instead of
  `std::fs`, so a slow disk read no longer blocks a runtime thread
  either. TLS moved from the blocking `rustls::Stream` wrapper to
  `tokio-rustls`'s async `TlsAcceptor`. The proxy relay and health
  checker (`proxy/mod.rs`) were rewritten the same way, using
  `tokio::net::TcpStream` and `tokio::time`.
- **Verified:** full regression pass across every earlier phase under
  the new model - static files (200), TLS handshake (confirmed
  TLSv1.3 / `TLS_AES_256_GCM_SHA384` again post-rewrite), proxy
  round-robin (A/B/A/B), health-check failover (all traffic correctly
  routed to the surviving upstream after a check cycle), and 80
  simultaneous client requests completing successfully against a
  single NWarp process.
- **Known limitation:** this is epoll via Tokio, not raw io_uring.
  io_uring (lower syscall overhead than epoll under very high
  connection counts) remains a possible future refinement, but Tokio's
  epoll-based reactor is already the same class of I/O model Nginx
  uses, and is a substantial, verified upgrade over Phase 1-3.5's
  thread-per-connection approach.

## Phase 5: HTTP/2 (done)

- **Library:** [h2](https://github.com/hyperium/h2) - the low-level
  HTTP/2 crate maintained by the same team behind `hyper`, built on
  Tokio. Hand-rolling HTTP/2 (binary framing, HPACK header
  compression, stream multiplexing, flow control) from scratch would
  be a multi-month effort on its own and a significant correctness/
  security risk; using the production-grade crate the wider Rust
  ecosystem already relies on is the same engineering call real
  servers make.
- **Negotiation:** HTTP/2 is offered via ALPN during the TLS handshake
  only (`tls::build_tls_config` now sets
  `alpn_protocols = ["h2", "http/1.1"]`). After `tokio-rustls` completes
  the handshake, the negotiated protocol
  (`tls_stream.get_ref().1.alpn_protocol()`) decides whether the
  connection is hopped to `http2::serve` or the existing HTTP/1.1
  `handle_generic` path - both live side by side on the same TLS port.
  Cleartext HTTP/2 (h2c) is out of scope; the plain HTTP port remains
  HTTP/1.1-only.
- **Bridging to existing logic:** rather than duplicating routing and
  proxying, `http2::handle_stream` converts an incoming h2
  `http::Request` into the same internal `Request` struct used by
  HTTP/1.1, calls the same `router::route` / `proxy` functions, then
  converts the resulting internal `Response` (or, for proxied
  requests, a parsed raw upstream response via the new
  `proxy::relay_raw` / `proxy::split_raw_response`) into an h2 response
  + data frame. Static file serving, routing, and proxying logic is
  not duplicated between HTTP/1.1 and HTTP/2 - only the wire-level
  adapters differ.
- **Verified:** `curl --http2 -v` against the TLS port shows the real
  ALPN negotiation (`ALPN: server accepted h2`, `using HTTP/2`) and
  actual H2 frames; confirmed `http_version: 2` (curl's own protocol
  report) for the welcome page, a 404, and a proxied request with the
  correct upstream body relayed through. HTTP/1.1 clients on the same
  port continue to work (`http_version: 1.1`), and plain HTTP (no TLS)
  is unaffected.
- **Known limitations (intentionally not hidden):**
  - h2c (cleartext HTTP/2) is not implemented.
  - Proxied requests still speak HTTP/1.1 to the upstream regardless of
    which protocol the client used to reach NWarp - the HTTP/2
    multiplexing benefit doesn't currently extend past NWarp itself.

## Planned phases

| Phase | Goal |
|---|---|
| 5.5 | HTTP/3 (QUIC) |
| 6 | WASM module system for request handlers (the "advanced feature" differentiator vs Apache/Nginx) |
| 7 | Config hot-reload, structured (OpenTelemetry) logging, `.deb`/`.rpm` packaging polish |

## Module map

```
src/
├── main.rs           entry point, CLI args, config load, builds the
│                     Tokio runtime (worker_threads-sized) and blocks on it
├── config/mod.rs      config file loading (incl. TLS + proxy_route settings)
├── http/
│   ├── request.rs      async request-line + header parsing, path sanitization
│   │                   (generic over any AsyncRead stream)
│   ├── response.rs      async status line + header + body serialization
│   │                   (generic over any AsyncWrite stream)
│   └── router.rs        async static file resolution (tokio::fs), MIME mapping
├── server/
│   ├── listener.rs      async HTTP accept loop, tokio::spawn per connection,
│   │                   builds the shared ProxyTable
│   └── connection.rs     shared async request/response cycle (handle_generic):
│                         checks proxy routes first, falls through to
│                         static file serving - used by both HTTP and HTTPS
├── proxy/mod.rs        ProxyTable, round-robin + health-tracked upstream
│                       selection, async background health checker, async relay
│                       (relay + relay_raw variants - see http2/mod.rs)
├── http2/mod.rs        HTTP/2 via the h2 crate: converts h2 streams to/from
│                       the internal Request/Response types, reusing the
│                       same router/proxy logic as HTTP/1.1
├── tls/mod.rs          rustls config (incl. ALPN), cert/key loading, async
│                       HTTPS accept loop via tokio-rustls, branches to
│                       HTTP/1.1 or HTTP/2 based on negotiated ALPN protocol
└── logging/mod.rs      access/error log writer
```
