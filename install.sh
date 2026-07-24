#!/usr/bin/env bash
#
# NWarp installer - builds the release binary and installs it
# system-wide following the same layout Apache/Nginx use.
#
# Usage: sudo ./install.sh

set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "Please run as root (sudo ./install.sh)"
  exit 1
fi

echo "== NWarp installer =="

# 1. Build release binary
echo "-> Building release binary..."
cargo build --release

# 2. Create dedicated system user (like www-data for Apache)
if ! id -u nwarp >/dev/null 2>&1; then
  echo "-> Creating system user 'nwarp'..."
  useradd --system --no-create-home --shell /usr/sbin/nologin nwarp
fi

# 3. Install binary
echo "-> Installing binary to /usr/sbin/nwarpd..."
install -Dm755 target/release/nwarpd /usr/sbin/nwarpd

# 4. Install config
echo "-> Installing config to /etc/nwarp/..."
mkdir -p /etc/nwarp
if [[ ! -f /etc/nwarp/nwarp.conf ]]; then
  cp configs/nwarp.conf /etc/nwarp/nwarp.conf
fi

# 5. Install default site content
echo "-> Installing default site to /var/www/nwarp-default..."
mkdir -p /var/www/nwarp-default
cp -r www/* /var/www/nwarp-default/
sed -i 's|document_root = .*|document_root = /var/www/nwarp-default|' /etc/nwarp/nwarp.conf

# 6. Log directory
echo "-> Creating /var/log/nwarp/..."
mkdir -p /var/log/nwarp
sed -i 's|access_log = .*|access_log = /var/log/nwarp/access.log|' /etc/nwarp/nwarp.conf
sed -i 's|error_log = .*|error_log = /var/log/nwarp/error.log|' /etc/nwarp/nwarp.conf

chown -R nwarp:nwarp /etc/nwarp /var/log/nwarp /var/www/nwarp-default

# 7. systemd service
echo "-> Installing systemd service..."
cp packaging/systemd/nwarp.service /etc/systemd/system/nwarp.service
systemctl daemon-reload

echo ""
echo "== Install complete =="
echo "Start it with:   sudo systemctl start nwarp"
echo "Enable on boot:  sudo systemctl enable nwarp"
echo "Check status:    sudo systemctl status nwarp"
echo "Config file:     /etc/nwarp/nwarp.conf"
echo "Default site:    /var/www/nwarp-default"
echo "Logs:            /var/log/nwarp/"
