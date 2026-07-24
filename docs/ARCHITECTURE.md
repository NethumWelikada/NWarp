# NWarp Architecture

*A project by Nethum Welikada, Master of Engineering in Internetworking,
Dalhousie University, Halifax, Nova Scotia, Canada.*

## Phase 1: Foundation (done)

- **Concurrency model:** thread-per-connection, bounded by a fixed-size
  thread pool (`worker_threads` in config, default 4). See
  `src/server/pool.rs`.
- **I/O:** blocking std TCP sockets (`std::net::TcpListener`). No async
  runtime yet.
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

## Planned phases

| Phase | Goal |
|---|---|
| 4 | Move from thread-per-connection to an epoll/io_uring event loop |
| 5 | HTTP/2, then HTTP/3 (QUIC) |
| 6 | WASM module system for request handlers (the "advanced feature" differentiator vs Apache/Nginx) |
| 7 | Config hot-reload, structured (OpenTelemetry) logging, `.deb`/`.rpm` packaging polish |

## Module map

```
src/
├── main.rs           entry point, CLI args, config path resolution
├── config/mod.rs      config file loading (incl. TLS + proxy_route settings)
├── http/
│   ├── request.rs      request-line + header parsing, path sanitization
│   │                   (generic over any Read stream)
│   ├── response.rs      status line + header + body serialization
│   │                   (generic over any Write stream)
│   └── router.rs        static file resolution, MIME type mapping
├── server/
│   ├── listener.rs      plain HTTP TCP bind + accept loop, spawns TLS thread,
│   │                   builds the shared ProxyTable
│   ├── pool.rs           fixed-size thread pool
│   └── connection.rs     shared request/response cycle (handle_generic):
│                         checks proxy routes first, falls through to
│                         static file serving - used by both HTTP and HTTPS
├── proxy/mod.rs        ProxyTable, round-robin + health-tracked upstream
│                       selection, background health checker, relay logic
├── tls/mod.rs          rustls config, cert/key loading, HTTPS accept loop
└── logging/mod.rs      access/error log writer
```
