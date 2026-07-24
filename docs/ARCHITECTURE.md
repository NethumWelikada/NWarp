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

## Planned phases

| Phase | Goal |
|---|---|
| 3 | Move from thread-per-connection to an epoll/io_uring event loop |
| 4 | Reverse proxy + load balancing |
| 5 | HTTP/2, then HTTP/3 (QUIC) |
| 6 | WASM module system for request handlers (the "advanced feature" differentiator vs Apache/Nginx) |
| 7 | Config hot-reload, structured (OpenTelemetry) logging, `.deb`/`.rpm` packaging polish |

## Module map

```
src/
├── main.rs           entry point, CLI args, config path resolution
├── config/mod.rs      config file loading (incl. TLS settings)
├── http/
│   ├── request.rs      request-line + header parsing, path sanitization
│   │                   (generic over any Read stream)
│   ├── response.rs      status line + header + body serialization
│   │                   (generic over any Write stream)
│   └── router.rs        static file resolution, MIME type mapping
├── server/
│   ├── listener.rs      plain HTTP TCP bind + accept loop, spawns TLS thread
│   ├── pool.rs           fixed-size thread pool
│   └── connection.rs     shared request/response cycle (handle_generic),
│                         used by both HTTP and HTTPS
├── tls/mod.rs          rustls config, cert/key loading, HTTPS accept loop
└── logging/mod.rs      access/error log writer
```
