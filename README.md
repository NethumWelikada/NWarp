# NWarp

A modern, high-performance HTTP web server written in Rust, built by
**Nethum Welikada**, Master of Engineering in Internetworking,
Dalhousie University, Halifax, Nova Scotia, Canada - engineered to go
beyond what Apache and Nginx offer, not just replicate it.

> All phases complete: static file serving, an async event loop
> (Tokio, epoll-based on Linux), TLS/HTTPS via rustls (TLSv1.3),
> HTTP/2 and HTTP/3 (QUIC, negotiated automatically), reverse proxy
> with round-robin load balancing and active health checks, a
> sandboxed WASM module system for request handlers, config
> hot-reload, structured JSON logging, and real `.deb` packaging. See
> [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full phase-by-phase
> writeup and honestly-documented limitations.

## Requirements

- Rust toolchain (`rustc` + `cargo`). Install via your package manager:
  ```bash
  sudo apt install rustc cargo      # Debian/Ubuntu
  sudo dnf install rust cargo       # Fedora
  ```
  Or via [rustup](https://rustup.rs) for the latest version.

## Quick start (run without installing)

```bash
git clone https://github.com/YOUR-USERNAME/nwarp.git
cd nwarp
cargo build --release
./target/release/nwarpd --config ./configs/nwarp.conf
```

Then visit **http://localhost:9090** - you should see the default NWarp
welcome page (served from `www/index.html`).

## Configuration

Edit `configs/nwarp.conf`:

```ini
host = 0.0.0.0
port = 9090
document_root = ./www
server_name = NWarp
index = index.html
worker_threads = 4
access_log = ./logs/access.log
error_log = ./logs/error.log

tls_enabled = false
tls_port = 9443
tls_cert = ./certs/dev-cert.pem
tls_key = ./certs/dev-key.pem
```

Point `document_root` at any folder of static files (HTML, CSS, JS,
images) and NWarp will serve it.

## HTTPS / TLS (Phase 2)

NWarp supports TLS via [rustls](https://github.com/rustls/rustls),
negotiating TLSv1.3 with modern cipher suites. The plain HTTP listener
and the HTTPS listener run at the same time on separate ports, so
enabling TLS doesn't disable HTTP.

**1. Generate a certificate.** For local development/testing, generate
a self-signed one:

```bash
./scripts/generate-dev-cert.sh
```

For a real deployment, use a certificate from a real CA (e.g.
[Let's Encrypt](https://letsencrypt.org)) instead - self-signed
certificates will show a browser warning and should never be used
publicly.

**2. Enable it in `configs/nwarp.conf`:**

```ini
tls_enabled = true
tls_port = 9443
tls_cert = ./certs/dev-cert.pem
tls_key = ./certs/dev-key.pem
```

**3. Restart NWarp**, then visit `https://localhost:9443` (your
browser will warn about the self-signed cert during local testing -
that's expected; proceed past it, or use a real CA cert to avoid the
warning entirely).

## HTTP/2 (Phase 5)

NWarp negotiates HTTP/2 automatically over HTTPS via ALPN - no
separate config flag needed. If TLS is enabled (see above), any
HTTP/2-capable client (modern browsers, `curl --http2`) that connects
gets HTTP/2 automatically; clients that don't support it fall back to
HTTP/1.1 on the same port, exactly as real-world servers behave.

```bash
curl -sk --http2 -v https://localhost:9443/ 2>&1 | grep "using HTTP"
# * using HTTP/2
```

Static file serving and reverse proxying both work over HTTP/2 -
verified end-to-end, including 404s and proxy round-robin.

**Current limitations (honest, so you don't hit surprises):**
- HTTP/2 is only available over TLS (`h2` via ALPN). Cleartext HTTP/2
  (`h2c`, used by some internal service-to-service traffic) is not
  implemented - the plain HTTP port (9090 by default) continues to
  speak HTTP/1.1 only.
- When proxying, NWarp still talks HTTP/1.1 to the upstream regardless
  of which protocol the client used to reach NWarp itself - this
  matches how most reverse proxies operate (client-facing and
  upstream-facing protocols are independent), but it's worth knowing
  the multiplexing benefit of HTTP/2 doesn't currently extend past
  NWarp to the upstream.

## HTTP/3 / QUIC (Phase 5.5)

NWarp also runs HTTP/3 over QUIC, using the [quinn](https://github.com/quinn-rs/quinn)
transport and [h3](https://github.com/hyperium/h3) framing crates - the
same stack the wider Rust ecosystem uses for QUIC/HTTP-3. It listens
on the same port number as the TLS/TCP listener (`tls_port`), just
over UDP instead of TCP, which is how real-world HTTP/3 deployments
work (the port number is shared; only the transport protocol differs).

Enabling it requires no extra config beyond TLS already being on -
whenever `tls_enabled = true`, the QUIC/HTTP-3 listener starts
automatically on `tls_port` alongside the TCP-based TLS listener.

**Verification note:** the `curl` build in this project's own test
environment doesn't have HTTP/3 support compiled in (common - it
requires a QUIC-capable TLS backend curl often isn't built with), so
this was verified with a small dedicated test client built from the
same `quinn`/`h3` crates instead, confirming a real QUIC handshake, a
200 response with the correct welcome-page body, and a WASM route
correctly invoked over HTTP/3.

**Current limitations (honest, so you don't hit surprises):**
- No `Alt-Svc` header is sent yet, so browsers won't automatically
  discover that HTTP/3 is available on this server - a client has to
  be told to use HTTP/3 directly (as the test client above does).
  Advertising `Alt-Svc: h3=":<port>"` on HTTP/1.1 and HTTP/2 responses
  is a natural follow-up.
- Same upstream-facing limitation as HTTP/2: proxied requests still
  speak HTTP/1.1 to the upstream regardless of the client's protocol.
- Request bodies aren't forwarded yet, consistent with every other
  phase's header-only request handling.

## Reverse proxy + load balancing (Phase 3, health checks in 3.5)

NWarp can proxy requests matching a path prefix to one or more
upstream servers, load-balanced round-robin, with active health
checks automatically routing around dead upstreams. This is opt-in -
if `configs/nwarp.conf` has no `proxy_route` lines, NWarp behaves
exactly like Phase 1/2 (static files only).

**Configure one or more routes:**

```ini
proxy_route /api = http://127.0.0.1:5001,http://127.0.0.1:5002
proxy_route /app = http://127.0.0.1:6000

health_check_interval = 5
health_check_timeout = 2
```

Any request whose path starts with `/api` is forwarded to whichever
*healthy* upstream is next in round-robin rotation. Every
`health_check_interval` seconds, NWarp attempts a TCP connection
(bounded by `health_check_timeout`) to each configured upstream and
marks it healthy or unhealthy accordingly - unhealthy upstreams are
skipped until a later check finds them reachable again. If every
upstream for a route is currently unhealthy, NWarp returns
`503 Service Unavailable` immediately rather than hanging on a
connection attempt to a known-dead host.

**Current limitations (honest, so you don't hit surprises):**
- Health checks are TCP-connectivity checks only (is the port
  reachable), not application-level checks (e.g. does `/health` return
  200). That's a reasonable follow-up but isn't built yet.
- Upstreams must be plain HTTP for now - proxying to a TLS upstream is
  a later phase.
- Request bodies (POST/PUT payloads) aren't forwarded yet, consistent
  with static-file-serving-only request parsing in Phase 1.

## WASM module system (Phase 6)

This is NWarp's core differentiator against Apache and Nginx: request
handlers can be sandboxed WebAssembly modules, written in any language
that compiles to WASM, instead of compiled C modules (Apache) or
embedded Lua/njs scripts (Nginx). NWarp runs modules with
[wasmi](https://github.com/wasmi-labs/wasmi), a pure-Rust, embeddable
WASM interpreter - no external runtime, no shelling out, no native
code execution outside the sandbox.

**Configure a route:**

```ini
wasm_route /hello = ./wasm/hello.wasm
```

Any request under `/hello` is handed to that module's `handle` export
instead of being served as a static file. A working example ships at
`wasm/hello.wasm` (with its `wasm/hello.wat` source alongside it for
transparency) - it reads the real requested path out of the incoming
request and echoes it back, proving the host is passing genuine
per-request data into the sandbox rather than returning a canned
string.

```bash
curl http://localhost:9090/hello/world
# Hello from a sandboxed WASM module! You requested: /hello/world
```

**The handler ABI** (documented in full in `src/wasm/mod.rs` and
`docs/ARCHITECTURE.md`): a compatible module exports `memory`,
`alloc(size) -> ptr`, and
`handle(method_ptr, method_len, path_ptr, path_len) -> i64` (packing a
response pointer and length). The response's first 2 bytes are the
HTTP status code (u16 little-endian); everything after is the body.

**Writing your own module:** any toolchain that emits a standard WASM
binary works, as long as it implements the ABI above. This repo's
`wasm/hello.wat` is written directly in WebAssembly Text format and
assembled with the `wat` crate specifically because this project's
own build environment didn't have a `wasm32-unknown-unknown` Rust
target available - if yours does, compiling a small `#![no_std]` Rust
crate to `wasm32-unknown-unknown` implementing the same three exports
works just as well and is generally more ergonomic than hand-writing
WAT.

**Current limitations (honest, so you don't hit surprises):**
- Fixed response content type (`text/plain; charset=utf-8`) for
  now - modules can't yet set arbitrary response headers.
- No request body support, and no host-provided imports (logging,
  outbound HTTP, key-value storage, etc.) - modules currently only
  receive the method and path, nothing else.
- A fresh WASM instance (fresh linear memory, fresh globals) is
  created per request for isolation - safe by default, but each
  request pays a small instantiation cost. Pooling/reusing instances
  across requests is a natural follow-up, not implemented here.
- A module that fails to load or compile is logged as a startup
  warning and its route is simply skipped (falls through to normal
  static file serving / 404) rather than crashing the server -
  verified by configuring a route pointing at a nonexistent file.

## Installing system-wide (like `apache2`/`nginx`)

This sets NWarp up the same way Apache/Nginx are installed: a dedicated
system user, binary in `/usr/sbin`, config in `/etc`, logs in
`/var/log`, and a systemd service. Two ways to do it:

**Option A - `.deb` package (Debian/Ubuntu):**

```bash
./packaging/build-deb.sh
sudo dpkg -i nwarp_0.1.0_amd64.deb
sudo systemctl enable --now nwarp
```

**Option B - install script (any distro with systemd):**

```bash
sudo ./install.sh
```

Then either way:

```bash
sudo systemctl start nwarp       # start it
sudo systemctl enable nwarp      # start on boot
sudo systemctl status nwarp      # check it's running
sudo systemctl stop nwarp        # stop it
sudo systemctl restart nwarp     # restart after config changes
```

**Note on config changes:** `proxy_route` and `wasm_route` entries
hot-reload automatically within a few seconds of editing
`/etc/nwarp/nwarp.conf` - no restart needed. Everything else (`host`,
`port`, `tls_*`, `worker_threads`) requires
`sudo systemctl restart nwarp` to take effect. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) Phase 7 for exactly what
does and doesn't hot-reload.

| What | Path |
|---|---|
| Binary | `/usr/sbin/nwarpd` |
| Config | `/etc/nwarp/nwarp.conf` |
| Default site | `/var/www/nwarp-default/` |
| Logs | `/var/log/nwarp/access.log`, `/var/log/nwarp/error.log` |
| systemd unit | `/etc/systemd/system/nwarp.service` |
| Runs as | dedicated `nwarp` system user (not root) |

To serve your own site, edit `document_root` in
`/etc/nwarp/nwarp.conf` to point at your files, then
`sudo systemctl restart nwarp`.

## Uninstalling

```bash
sudo systemctl stop nwarp
sudo systemctl disable nwarp
sudo rm /etc/systemd/system/nwarp.service
sudo rm /usr/sbin/nwarpd
sudo rm -rf /etc/nwarp /var/log/nwarp /var/www/nwarp-default
sudo userdel nwarp
sudo systemctl daemon-reload
```

## Project structure

```
nwarp/
├── src/                  Rust source (see docs/ARCHITECTURE.md)
├── configs/nwarp.conf     default config
├── www/                   default site content served out of the box
├── certs/                 TLS certs go here (gitignored, generate your own)
├── scripts/generate-dev-cert.sh   self-signed dev cert generator
├── packaging/
│   ├── systemd/            nwarp.service unit file
│   ├── debian/             control, postinst, postrm
│   └── build-deb.sh        builds a real .deb package (verified working)
├── docs/ARCHITECTURE.md   design notes + roadmap
├── src/proxy/mod.rs       reverse proxy + round-robin load balancing
├── src/http2/mod.rs       HTTP/2 (h2 crate bridge to internal Request/Response)
├── src/http3/mod.rs       HTTP/3 over QUIC (quinn + h3 crates)
├── src/wasm/mod.rs        WASM module system (wasmi) - the Apache/Nginx differentiator
├── wasm/                  WASM handler modules go here (hello.wasm example + .wat source)
├── install.sh             system-wide installer (Tier 1 packaging)
└── LICENSE                MIT (with attribution)
```

## Roadmap

All planned phases are complete. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full phase-by-phase
writeup, honestly-documented limitations at each stage, and a short
list of possible future directions beyond the original phase plan.

## License

MIT, with attribution required to Nethum Welikada. See [LICENSE](LICENSE).
