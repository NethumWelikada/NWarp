# NWarp

A modern HTTP web server written in Rust - engineered to go beyond
what Apache and Nginx offer, not just replicate them: an async
epoll-based event loop, TLS 1.3, HTTP/2, HTTP/3 (QUIC), a reverse
proxy with health-checked load balancing, config hot-reload, and a
sandboxed WebAssembly module system for request handlers.

Built by [Nethum Welikada](https://github.com/NethumWelikada), Master
of Engineering in Internetworking, Dalhousie University, Halifax,
Nova Scotia, Canada.

---

## Install

### Option A - `.deb` package (Debian/Ubuntu, recommended)

```bash
git clone https://github.com/NethumWelikada/NWarp.git
cd NWarp
./packaging/build-deb.sh
sudo dpkg -i nwarp_0.1.0_amd64.deb
sudo systemctl enable --now nwarp
```

Check it's running:

```bash
sudo systemctl status nwarp
curl http://localhost:9090/
```

### Option B - build and run manually (any Linux distro)

Requires the Rust toolchain (`rustc` + `cargo`):

```bash
sudo apt install rustc cargo        # Debian/Ubuntu
sudo dnf install rust cargo         # Fedora
```

Then:

```bash
git clone https://github.com/NethumWelikada/NWarp.git
cd NWarp
cargo build --release
./target/release/nwarpd --config ./configs/nwarp.conf
```

Visit **http://localhost:9090** - you should see the NWarp welcome page.

### Option C - install script (any systemd Linux, no `.deb`)

```bash
git clone https://github.com/NethumWelikada/NWarp.git
cd NWarp
sudo ./install.sh
sudo systemctl enable --now nwarp
```

All three options install the same way Apache/Nginx do: a dedicated
`nwarp` system user, binary in `/usr/sbin`, config in `/etc/nwarp`,
logs in `/var/log/nwarp`, default site in `/var/www/nwarp-default`,
and a systemd service.

---

## Features

- **Async event loop** (Tokio, epoll on Linux) - handles many
  thousands of concurrent connections without a thread per connection
- **HTTP/1.1, HTTP/2, and HTTP/3** - HTTP/2 and HTTP/3 negotiate
  automatically over TLS, no extra config needed
- **TLS 1.3** via [rustls](https://github.com/rustls/rustls), a
  memory-safe Rust TLS implementation
- **Reverse proxy** with round-robin load balancing and active
  upstream health checks - dead upstreams are automatically skipped
- **Sandboxed WebAssembly request handlers** - write handlers in any
  language that compiles to WASM, run in a real sandbox
  ([wasmi](https://github.com/wasmi-labs/wasmi)) instead of compiled C
  modules (Apache) or embedded scripting (Nginx)
- **Config hot-reload** - edit `proxy_route`/`wasm_route` entries and
  they apply within seconds, no restart
- **Structured JSON logging** - line-delimited JSON access/error logs,
  directly ingestible by an OpenTelemetry Collector, Vector, or Fluent
  Bit
- **Directory-traversal protection**, custom error pages, and a
  config-driven document root

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how each of these
is implemented and what their current limitations are.

---

## Configuration

Edit `/etc/nwarp/nwarp.conf` (system install) or
`configs/nwarp.conf` (manual build):

```ini
host = 0.0.0.0
port = 9090
document_root = /var/www/nwarp-default
server_name = NWarp
index = index.html
worker_threads = 4
access_log = /var/log/nwarp/access.log
error_log = /var/log/nwarp/error.log

# TLS / HTTPS - also enables HTTP/2 and HTTP/3 automatically
tls_enabled = false
tls_port = 9443
tls_cert = ./certs/dev-cert.pem
tls_key = ./certs/dev-key.pem

# Reverse proxy + load balancing (optional, comment out to disable)
# proxy_route /api = http://127.0.0.1:5001,http://127.0.0.1:5002
health_check_interval = 5
health_check_timeout = 2

# WASM request handlers (optional)
# wasm_route /hello = ./wasm/hello.wasm
```

`proxy_route` and `wasm_route` changes hot-reload automatically within
a few seconds - no restart needed. Everything else (`host`, `port`,
`tls_*`, `worker_threads`) requires
`sudo systemctl restart nwarp` to take effect.

To serve your own site: point `document_root` at your files and
restart, or run manually with `--config` pointing at your own config
file.

---

## HTTPS, HTTP/2, and HTTP/3

Generate a certificate - a self-signed one for local testing:

```bash
./scripts/generate-dev-cert.sh
```

For a real deployment, use a certificate from a real CA (e.g.
[Let's Encrypt](https://letsencrypt.org)) instead.

Enable it in the config:

```ini
tls_enabled = true
tls_port = 9443
tls_cert = ./certs/dev-cert.pem
tls_key = ./certs/dev-key.pem
```

Restart, then visit `https://localhost:9443`. HTTP/2 negotiates
automatically via ALPN for any HTTP/2-capable client:

```bash
curl -k --http2 -v https://localhost:9443/ 2>&1 | grep "using HTTP"
# * using HTTP/2
```

HTTP/3 (QUIC) runs on the same port number, over UDP, automatically
whenever TLS is enabled - no separate flag. Note: HTTP/3 isn't
discoverable via `Alt-Svc` yet, so a client needs to be told to use it
directly (most `curl` builds don't have QUIC support compiled in;
browsers and dedicated HTTP/3 clients do).

---

## Reverse proxy and load balancing

```ini
proxy_route /api = http://127.0.0.1:5001,http://127.0.0.1:5002
health_check_interval = 5
health_check_timeout = 2
```

Requests under `/api` are load-balanced round-robin across the listed
upstreams. Every `health_check_interval` seconds, NWarp checks each
upstream's TCP reachability and automatically routes around any that
are down. If every upstream for a route is unreachable, NWarp returns
`503 Service Unavailable`.

Current scope: upstreams must be plain HTTP (not HTTPS), health checks
are TCP-reachability only (not an application-level `/health` check),
and request bodies aren't forwarded yet.

---

## WASM request handlers

NWarp's core differentiator: request handlers as sandboxed WebAssembly
modules instead of compiled C modules (Apache) or embedded Lua/njs
(Nginx).

```ini
wasm_route /hello = ./wasm/hello.wasm
```

A working example ships at `wasm/hello.wasm` (source at
`wasm/hello.wat`) - it echoes the real requested path back, proving
the host passes genuine per-request data into the sandbox:

```bash
curl http://localhost:9090/hello/world
# Hello from a sandboxed WASM module! You requested: /hello/world
```

**Handler ABI:** a compatible module exports `memory`,
`alloc(size) -> ptr`, and
`handle(method_ptr, method_len, path_ptr, path_len) -> i64` (packing a
response pointer and length). The response's first 2 bytes are the
HTTP status code (u16 little-endian); everything after is the body.
Full details in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Current scope: fixed response content type, no request body or
host-provided imports (logging, outbound HTTP, storage) yet, and a
fresh sandboxed instance is created per request for isolation.

---

## Uninstalling

**If installed via `.deb`:**

```bash
sudo systemctl stop nwarp
sudo dpkg -P nwarp
```

**If installed via `install.sh`:**

```bash
sudo systemctl stop nwarp
sudo systemctl disable nwarp
sudo rm /etc/systemd/system/nwarp.service
sudo rm /usr/sbin/nwarpd
sudo rm -rf /etc/nwarp /var/log/nwarp /var/www/nwarp-default
sudo userdel nwarp
sudo systemctl daemon-reload
```

---

## Project structure

```
NWarp/
├── src/                    Rust source - see docs/ARCHITECTURE.md
│   ├── http/                 HTTP/1.1 parsing, response building, static routing
│   ├── http2/                 HTTP/2 (h2 crate)
│   ├── http3/                 HTTP/3 over QUIC (quinn + h3 crates)
│   ├── proxy/                 reverse proxy, load balancing, health checks
│   ├── wasm/                  WASM module system (wasmi)
│   ├── tls/                   TLS config, cert loading
│   ├── server/                async accept loops, connection handling, hot-reload
│   ├── config/                config file loading
│   └── logging/                structured JSON logging
├── configs/nwarp.conf       default config
├── www/                     default site content
├── wasm/                    example WASM handler (hello.wasm + .wat source)
├── scripts/generate-dev-cert.sh   self-signed dev TLS cert generator
├── packaging/
│   ├── systemd/                nwarp.service unit file
│   ├── debian/                 control, postinst, postrm
│   └── build-deb.sh            builds a real, installable .deb
├── install.sh               manual system-wide installer
├── docs/ARCHITECTURE.md     full technical design writeup
└── LICENSE                  MIT (with attribution)
```

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full,
phase-by-phase technical design writeup, including what each feature's
current limitations are and where they'd be extended next.

## License

MIT, with attribution required to
[Nethum Welikada](https://github.com/NethumWelikada). See
[LICENSE](LICENSE).

## Author

**[Nethum Welikada](https://github.com/NethumWelikada)**
Master of Engineering in Internetworking
Dalhousie University, Halifax, Nova Scotia, Canada

