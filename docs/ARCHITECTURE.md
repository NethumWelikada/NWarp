# NWarp Architecture

*A project by Nethum Welikada, Master of Engineering in Internetworking,
Dalhousie University, Halifax, Nova Scotia, Canada.*

## Phase 1 (current)

- **Concurrency model:** thread-per-connection, bounded by a fixed-size
  thread pool (`worker_threads` in config, default 4). See
  `src/server/pool.rs`.
- **I/O:** blocking std TCP sockets (`std::net::TcpListener`). No async
  runtime yet.
- **Serving:** static files only, resolved under `document_root`.
  Directory traversal (`..`) is blocked at two layers: string sanitation
  in `http::request::sanitize_path`, and a canonicalized-path check in
  `http::router::path_is_within`.
- **Config:** flat `key = value` file, parsed in `config/mod.rs`. No
  external parsing crate - keeps the project dependency-free so it
  builds anywhere with just `rustc`/`cargo`.
- **Logging:** flat-file access/error logs, one line per request.

## Planned phases

| Phase | Goal |
|---|---|
| 2 | Move from thread-per-connection to an epoll/io_uring event loop |
| 3 | TLS termination (rustls) |
| 4 | Reverse proxy + load balancing |
| 5 | HTTP/2, then HTTP/3 (QUIC) |
| 6 | WASM module system for request handlers (the "advanced feature" differentiator vs Apache/Nginx) |
| 7 | Config hot-reload, structured (OpenTelemetry) logging, `.deb`/`.rpm` packaging polish |

## Module map

```
src/
├── main.rs           entry point, CLI args, config path resolution
├── config/mod.rs      config file loading
├── http/
│   ├── request.rs      request-line + header parsing, path sanitization
│   ├── response.rs      status line + header + body serialization
│   └── router.rs        static file resolution, MIME type mapping
├── server/
│   ├── listener.rs      TCP bind + accept loop
│   ├── pool.rs           fixed-size thread pool
│   └── connection.rs     per-connection request/response cycle
└── logging/mod.rs      access/error log writer
```
