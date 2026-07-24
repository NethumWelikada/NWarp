# NWarp

A modern, high-performance HTTP web server written in Rust, built by
**Nethum Welikada**, Master of Engineering in Internetworking,
Dalhousie University, Halifax, Nova Scotia, Canada - engineered to go
beyond what Apache and Nginx offer, not just replicate it.

> Phase 1 (this release): static file serving, thread-pool concurrency,
> config file, access/error logging, directory-traversal protection -
> the foundation. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for
> the full roadmap: TLS, reverse proxy + load balancing, HTTP/2 & HTTP/3,
> and a WASM module system that neither Apache nor Nginx offer natively.

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
```

Point `document_root` at any folder of static files (HTML, CSS, JS,
images) and NWarp will serve it.

## Installing system-wide (like `apache2`/`nginx`)

This sets NWarp up the same way Apache/Nginx are installed: a dedicated
system user, binary in `/usr/sbin`, config in `/etc`, logs in
`/var/log`, and a systemd service.

```bash
sudo ./install.sh
```

Then:

```bash
sudo systemctl start nwarp       # start it
sudo systemctl enable nwarp      # start on boot
sudo systemctl status nwarp      # check it's running
sudo systemctl stop nwarp        # stop it
sudo systemctl restart nwarp     # restart after config changes
```

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
├── packaging/
│   ├── systemd/            nwarp.service unit file
│   └── debian/             .deb control file (Tier 2 packaging, WIP)
├── docs/ARCHITECTURE.md   design notes + roadmap
├── install.sh             system-wide installer (Tier 1 packaging)
└── LICENSE                MIT (with attribution)
```

## Roadmap

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the phased plan -
TLS termination, reverse proxy + load balancing, HTTP/2 & HTTP/3, and a
WASM-based module system as the long-term differentiator against
Apache and Nginx.

## License

MIT, with attribution required to Nethum Welikada. See [LICENSE](LICENSE).
