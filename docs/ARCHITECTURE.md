# NWarp Architecture

A technical design writeup of how NWarp works, why each major
decision was made, and - just as importantly - what each feature's
current limitations are. Nothing here is marked "done" without having
been built, compiled, and tested against real traffic; every
limitation listed is a genuine, current scope boundary, not modesty.

Built by [Nethum Welikada](https://github.com/NethumWelikada), Master
of Engineering in Internetworking, Dalhousie University, Halifax,
Nova Scotia, Canada.

---

## Overview

NWarp is a web server written in Rust, structured around a small set
of design goals:

- **Memory safety** throughout the request path - no C dependencies
  for TLS ([rustls](https://github.com/rustls/rustls)), HTTP/2
  ([h2](https://github.com/hyperium/h2)), or HTTP/3
  ([quinn](https://github.com/quinn-rs/quinn) /
  [h3](https://github.com/hyperium/h3))
- **One routing/handling core, many protocols** - HTTP/1.1, HTTP/2,
  and HTTP/3 all convert incoming requests into the same internal
  `Request` type and run through the same static-file/proxy/WASM
  routing logic, rather than three separate implementations
- **Async by default** - every connection is a fast Tokio task
  multiplexed over a small, fixed pool of OS threads (epoll on Linux),
  not a thread per connection
- **A real plugin model, sandboxed** - request handlers can be
  WebAssembly modules instead of compiled C modules or embedded
  scripting, running in an actual sandbox
  ([wasmi](https://github.com/wasmi-labs/wasmi))

## Module map

```
src/
├── main.rs           entry point, CLI args, config load, builds the
│                     Tokio runtime (worker_threads-sized) and blocks on it
├── config/mod.rs      config file loading (host/port/TLS/proxy/wasm settings)
├── http/
│   ├── request.rs      async request-line + header parsing, path sanitization
│   │                   (generic over any AsyncRead stream)
│   ├── response.rs      async status line + header + body serialization
│   │                   (generic over any AsyncWrite stream)
│   └── router.rs        async static file resolution (tokio::fs), MIME mapping
├── server/
│   ├── listener.rs      async HTTP accept loop, tokio::spawn per connection,
│   │                   builds ArcSwap-wrapped ProxyTable/WasmTable, spawns
│   │                   the config hot-reload watcher
│   └── connection.rs     shared async request/response cycle (handle_generic):
│                         checks proxy routes first, then WASM routes, then
│                         falls through to static file serving - used by
│                         both plain HTTP and HTTPS
├── proxy/mod.rs        ProxyTable, round-robin + health-tracked upstream
│                       selection, async background health checker, async relay
├── http2/mod.rs        HTTP/2 via the h2 crate: converts h2 streams to/from
│                       the internal Request/Response types, reusing the
│                       same router/proxy/wasm logic as HTTP/1.1
├── http3/mod.rs        HTTP/3 over QUIC via quinn + h3: separate QUIC-scoped
│                       rustls config, same routing/proxy/wasm bridge pattern
├── wasm/mod.rs         WASM module system (wasmi): route table, module
│                       compilation, per-request instantiation + invocation
├── tls/mod.rs          rustls config (incl. ALPN), cert/key loading, async
│                       HTTPS accept loop, branches to HTTP/1.1 or HTTP/2
│                       based on the negotiated ALPN protocol
└── logging/mod.rs      structured JSON access/error log writer
```

---

## TLS / HTTPS

- **Library:** [rustls](https://github.com/rustls/rustls), a
  memory-safe TLS implementation in pure Rust - no OpenSSL C bindings,
  unlike Apache/Nginx's default TLS stack.
- **Protocol:** negotiates TLS 1.3 with modern AEAD cipher suites
  (verified via live handshake: `TLS_AES_256_GCM_SHA384` / X25519).
- **Design:** the HTTPS accept loop (`src/tls/mod.rs`) runs as its own
  Tokio task alongside the plain-HTTP listener - both active at once
  on separate ports (`port` / `tls_port`), matching how Apache/Nginx
  run HTTP and HTTPS virtual hosts side by side.
- **Code reuse:** `http::request::Request::parse` and
  `http::response::Response::send` are generic over any
  `AsyncRead`/`AsyncWrite` stream, so the exact same routing and
  logging path (`server::connection::handle_generic`) serves both a
  plain `TcpStream` and a TLS-wrapped stream - no duplicated
  request-handling logic between HTTP and HTTPS.
- **Certs:** loaded from PEM files (`tls_cert` / `tls_key` in config),
  supporting both PKCS#8 and PKCS#1 (RSA) private key formats. A dev
  self-signed cert generator ships at `scripts/generate-dev-cert.sh`;
  production deployments should use a real CA (e.g. Let's Encrypt).

## HTTP/2

- **Library:** [h2](https://github.com/hyperium/h2), the low-level
  HTTP/2 crate maintained by the `hyper` team, built on Tokio.
  Hand-rolling HTTP/2 (binary framing, HPACK header compression,
  stream multiplexing, flow control) would be a significant
  correctness and security risk on its own; using the production-grade
  crate the wider Rust ecosystem relies on is the standard engineering
  choice here.
- **Negotiation:** HTTP/2 is offered via ALPN during the TLS handshake
  (`tls::build_tls_config` sets `alpn_protocols = ["h2", "http/1.1"]`).
  The negotiated protocol decides whether the connection is handed to
  `http2::serve` or the HTTP/1.1 path - both live side by side on the
  same TLS port. Cleartext HTTP/2 (h2c) is out of scope; the plain
  HTTP port remains HTTP/1.1-only.
- **Bridging:** `http2::handle_stream` converts an incoming h2
  `http::Request` into the same internal `Request` struct used by
  HTTP/1.1, calls the same `router::route` / `proxy` / `wasm`
  functions, then converts the response into an h2 response + data
  frame. No routing or handling logic is duplicated between HTTP/1.1
  and HTTP/2 - only the wire-level adapter differs.
- **Verified:** `curl --http2 -v` against the TLS port shows real ALPN
  negotiation (`ALPN: server accepted h2`, `using HTTP/2`) and actual
  H2 frames; confirmed `http_version: 2` for static pages, 404s, and
  proxied requests with the correct upstream body relayed through.
- **Current limitations:**
  - h2c (cleartext HTTP/2) is not implemented.
  - Proxied requests still speak HTTP/1.1 to the upstream regardless
    of which protocol the client used to reach NWarp.

## HTTP/3 (QUIC)

- **Stack:** [quinn](https://github.com/quinn-rs/quinn) for the QUIC
  transport layer, [h3](https://github.com/hyperium/h3) +
  [h3-quinn](https://github.com/hyperium/h3) for HTTP/3 framing -
  the same crates the wider Rust ecosystem uses for QUIC/H3.
- **TLS:** QUIC mandates TLS 1.3, so `http3::build_quic_tls_config`
  builds a dedicated rustls config (TLS 1.3 only, ALPN fixed to `h3`)
  using the same cert/key as the TCP/TLS listener - one certificate,
  two independent ALPN-scoped configs (TCP: h2/http1.1, QUIC: h3).
- **Listener:** binds the same port number as `tls_port`, over UDP -
  this mirrors real-world HTTP/3 deployments, where the port number is
  shared between TCP and UDP, and normally discovered via an
  `Alt-Svc` header (not yet implemented here - see limitations). Runs
  automatically whenever `tls_enabled = true`; no separate config flag.
- **Bridging:** identical pattern to HTTP/2 - `h3` streams convert to
  the same internal `Request` type, route through the same
  `proxy`/`wasm`/`router` functions, and the response goes back via
  `h3`'s `RequestStream` API.
- **Verified:** most default `curl` builds don't have HTTP/3 support
  compiled in (no QUIC-capable TLS backend), so verification used a
  small dedicated test client built from the same `quinn`/`h3` crates
  - confirming a genuine QUIC handshake, a `200 OK` response with the
  correct page body, and a WASM route correctly invoked over HTTP/3.
- **Current limitations:**
  - No `Alt-Svc` header advertisement yet, so clients must be told to
    use HTTP/3 directly rather than discovering it automatically.
  - Proxied requests still speak HTTP/1.1 to the upstream, same as HTTP/2.

## Reverse proxy and load balancing

- **Config:** `proxy_route <prefix> = <upstream1>,<upstream2>,...`
  lines. Fully opt-in - an empty list (the default) means every
  request is served as a static file, unchanged.
- **Matching:** longest-prefix match across all configured routes,
  checked before static file resolution.
- **Load balancing:** round-robin via a per-route atomic counter, safe
  to share across many concurrent tasks without locking.
- **Relay:** opens a fresh TCP connection to the chosen upstream per
  request, rebuilds the request line and headers (forwarding
  `X-Forwarded-For`, overriding `Host`), and streams the raw upstream
  response back byte-for-byte - verified against two real upstream
  servers with alternating round-robin.
- **Current limitations:**
  - Upstreams must be plain HTTP; proxying to a TLS-terminated
    upstream is not implemented.
  - Request bodies are not forwarded yet.

## Upstream health checks

- **Model:** each upstream target carries a lock-free healthy flag,
  read on every request and written by a background health-check task.
- **Check:** every `health_check_interval` seconds, a TCP connect
  attempt (bounded by `health_check_timeout`) determines reachability.
  This is a connectivity check, not an application-level check.
- **Selection:** round-robin picks only among currently healthy
  targets; if every target for a route is unhealthy, NWarp returns
  `503 Service Unavailable` immediately rather than attempting a
  connection to a known-dead host.
- **Verified:** killed one of two live upstreams mid-test - after one
  health-check cycle, all traffic correctly routed to the survivor;
  killing both correctly returned `503`, not `502`.
- **Current limitations:**
  - TCP-level reachability only, not an HTTP-level check (e.g. hitting
    a `/health` endpoint and checking for `200`).
  - On cold start, every upstream is assumed healthy until the first
    check completes.

## Async event loop

- **Runtime:** [Tokio](https://tokio.rs)'s multi-threaded scheduler,
  using epoll as its I/O reactor on Linux. Every connection becomes a
  fast Tokio task rather than an OS thread -
  `worker_threads` in config directly sizes the runtime's thread pool.
- **Why it matters:** under a thread-per-connection model, every open
  connection holds an entire OS thread for as long as it's open, even
  while idle. Under Tokio, a connection's task yields at every
  `.await` point instead of blocking a thread, so a small, fixed
  number of OS threads can service many thousands of concurrent
  connections - the same class of model Nginx's worker processes use
  internally.
- **Verified:** full regression pass across every feature under the
  async model (static files, TLS, proxy round-robin, health-check
  failover), plus 80+ simultaneous client requests completing
  successfully against a single process.
- **Current limitation:** this is epoll via Tokio, not raw io_uring.
  io_uring (lower syscall overhead under very high connection counts)
  remains a possible future refinement.

## WASM module system

NWarp's core differentiator against Apache and Nginx: request handlers
as sandboxed WebAssembly modules, rather than compiled C modules
(Apache) or embedded Lua/njs scripts (Nginx).

- **Runtime:** [wasmi](https://github.com/wasmi-labs/wasmi), a
  pure-Rust, embeddable WASM interpreter. Chosen deliberately over a
  JIT-compiling runtime like `wasmtime`: for a request-handler plugin
  model where each invocation is small and short-lived, an
  interpreter's per-call overhead is an acceptable trade-off for a
  dramatically lighter, pure-Rust dependency tree with no C toolchain
  requirement. `wasmi` is used in production systems (e.g.
  Parity/Substrate blockchain runtimes) for exactly this kind of
  embeddable-with-minimal-footprint use case.
- **Config:** `wasm_route <prefix> = <path/to/module.wasm>` lines. A
  module compiles once at startup; each request gets a fresh sandboxed
  instance for isolation, so one request's module state can never leak
  into another's. A module that fails to load or compile is logged as
  a startup warning and its route is skipped rather than crashing the
  server.
- **Handler ABI:** a compatible module exports:
  - `memory` - linear memory the host reads/writes into
  - `alloc(size: i32) -> i32` - bump-allocate `size` bytes, return a pointer
  - `handle(method_ptr, method_len, path_ptr, path_len) -> i64` -
    packs `(response_ptr << 32) | response_len` into the return value

  The response bytes are `[status: u16 little-endian][body: remaining bytes]`.
  Full contract in `src/wasm/mod.rs`.
- **Routing precedence:** proxy routes are checked first, then WASM
  routes, then static file serving as the final fallback.
- **Example module:** `wasm/hello.wasm` (source at `wasm/hello.wat`).
  Written directly in WebAssembly Text format and assembled with the
  [`wat`](https://github.com/bytecodealliance/wasm-tools) crate - a
  pure-Rust WAT parser/encoder that runs on any host target, requiring
  no `wasm32-unknown-unknown` Rust toolchain. The resulting `.wasm` is
  a genuine, valid WASM binary either way; `wasmi` has no way to tell
  (or care) whether a module came from Rust, C, AssemblyScript, or
  hand-written WAT. Any toolchain that emits a standard WASM binary
  implementing the ABI above works - including a `#![no_std]` Rust
  crate compiled to `wasm32-unknown-unknown`, if you have that target
  installed.
- **Verified:** the example module reads the actual requested path out
  of guest memory (written there by the host from the real incoming
  request) and echoes it back - confirmed with different request paths
  producing different response bodies, proving the host passes genuine
  per-request data into the sandbox. Confirmed working over plain
  HTTP, HTTPS, HTTP/2, and HTTP/3.
- **Current limitations:**
  - Fixed response content type (`text/plain; charset=utf-8`) - no
    per-response header control from the module yet.
  - No request body support, and no host-provided imports (logging,
    outbound HTTP, storage, etc.) - modules only receive method + path.
  - A fresh instance is created per request (safe, isolated) rather
    than a pooled/reused instance (faster, but requires careful state
    reset).

## Config hot-reload

- **Model:** the proxy and WASM routing tables are wrapped in
  [`arc-swap`](https://github.com/vorner/arc-swap), giving lock-free
  reads on every request and an atomic swap on reload.
- **Watcher:** a background task polls the config file's modification
  time every 3 seconds. On change, it reloads `proxy_route` and
  `wasm_route` entries into fresh tables and swaps them in - in-flight
  requests are unaffected, since each request loads its own snapshot
  at the start of the request.
- **Verified:** started a server with no proxy routes (confirmed
  `404`), appended a `proxy_route` line to the live config file while
  the server kept running, and confirmed the new route worked within
  seconds - with the process ID unchanged throughout, proving no
  restart occurred.
- **Deliberately not hot-reloaded:** `host`, `port`, `tls_*`,
  `worker_threads` - these are bound into already-listening sockets
  and an already-sized runtime; changing them safely requires a
  restart.
- **Current limitation:** each reload that changes proxy routes spawns
  a fresh health-checker task for the new table; the old table's
  health-checker keeps running harmlessly in the background (checking
  now-unreferenced targets), since there's no handle to cancel it. A
  small, bounded cost per reload, not unbounded growth in normal
  operation.

## Structured logging

- Every log line is a single JSON object (line-delimited JSON), built
  with `serde`/`serde_json` for correctness.
- **Access log fields:** `timestamp` (unix epoch seconds), `level`,
  `event`, `method`, `path`, `status`, `peer`, `server`.
  **Error log fields:** `timestamp`, `level`, `event`, `message`, `server`.
- **Scope, stated precisely:** this is structured JSON logging that an
  OpenTelemetry Collector's `filelog` receiver (or Vector, Fluent Bit,
  etc.) can ingest directly without a custom parser - it is *not* the
  OpenTelemetry SDK, does not export via OTLP, and has no trace/span
  context propagation. Full OTLP export is a reasonable follow-up.
- **Verified:** captured real access log output and parsed every line
  with a JSON parser to confirm it's genuinely valid JSON.

## Packaging

- `packaging/build-deb.sh` assembles a real Debian package tree
  (binary in `/usr/sbin`, config in `/etc/nwarp`, default site in
  `/var/www/nwarp-default`, systemd unit, bundled WASM examples) and
  builds it with `dpkg-deb`.
- `packaging/debian/postinst` creates the dedicated `nwarp` system user
  and reloads systemd; `postrm` cleans up on purge.
- **Verified:** built the package, inspected it with `dpkg-deb
  --info`/`--contents`, then actually installed it with `dpkg -i` -
  confirmed the binary runs from `/usr/sbin/nwarpd`, the config lands
  correctly, and the system user is created - then purged it cleanly.
- `install.sh` provides an alternative, non-`.deb` install path for
  other distros.

---

## Possible future directions

Ideas beyond the current feature set, not commitments:

- Health checks upgraded from TCP-reachability to application-level
  (HTTP `/health` endpoint checks)
- `Alt-Svc` header advertisement so browsers auto-discover HTTP/3
- WASM handler ABI: response headers, request bodies, host-provided
  imports (logging, outbound HTTP, KV storage), pooled/reused instances
- io_uring as an alternative I/O backend to epoll
- Full OTLP export via the `opentelemetry` crate
- `.rpm` packaging alongside `.deb`

---

## Author

**[Nethum Welikada](https://github.com/NethumWelikada)**
Master of Engineering in Internetworking
Dalhousie University, Halifax, Nova Scotia, Canada
