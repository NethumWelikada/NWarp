#!/usr/bin/env bash
#
# Builds a real .deb package for NWarp using dpkg-deb (no cargo-deb
# dependency required). Tested to produce a genuinely installable
# package - see docs/ARCHITECTURE.md Phase 7 for verification notes.
#
# Usage: ./packaging/build-deb.sh

set -euo pipefail

VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d '"' -f2)
ARCH=$(dpkg --print-architecture 2>/dev/null || echo amd64)
PKG_NAME="nwarp"
BUILD_DIR="/tmp/${PKG_NAME}-deb-build"
DEB_FILE="${PKG_NAME}_${VERSION}_${ARCH}.deb"

echo "== Building NWarp release binary =="
cargo build --release

echo "== Assembling package tree =="
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/DEBIAN"
mkdir -p "$BUILD_DIR/usr/sbin"
mkdir -p "$BUILD_DIR/etc/nwarp"
mkdir -p "$BUILD_DIR/etc/nwarp/wasm"
mkdir -p "$BUILD_DIR/var/www/nwarp-default"
mkdir -p "$BUILD_DIR/etc/systemd/system"

# Binary
install -m755 target/release/nwarpd "$BUILD_DIR/usr/sbin/nwarpd"

# Config (system paths baked in, matching install.sh's Tier 1 layout)
sed \
  -e 's|document_root = .*|document_root = /var/www/nwarp-default|' \
  -e 's|access_log = .*|access_log = /var/log/nwarp/access.log|' \
  -e 's|error_log = .*|error_log = /var/log/nwarp/error.log|' \
  configs/nwarp.conf > "$BUILD_DIR/etc/nwarp/nwarp.conf"

# Default site + WASM examples
cp -r www/* "$BUILD_DIR/var/www/nwarp-default/"
cp wasm/*.wasm wasm/*.wat "$BUILD_DIR/etc/nwarp/wasm/" 2>/dev/null || true

# systemd unit
cp packaging/systemd/nwarp.service "$BUILD_DIR/etc/systemd/system/nwarp.service"

# DEBIAN control metadata
cat > "$BUILD_DIR/DEBIAN/control" << EOF
Package: nwarp
Version: ${VERSION}
Section: httpd
Priority: optional
Architecture: ${ARCH}
Maintainer: Nethum Welikada
Description: NWarp - a modern HTTP web server written in Rust
 NWarp is engineered to go beyond what Apache and Nginx offer natively:
 an async epoll-based event loop, TLS 1.3, HTTP/2, HTTP/3 (QUIC),
 reverse proxy with health-checked load balancing, and a sandboxed
 WASM module system for request handlers. Developed by Nethum
 Welikada, Master of Engineering in Internetworking, Dalhousie
 University, Halifax, Nova Scotia, Canada.
EOF

cp packaging/debian/postinst "$BUILD_DIR/DEBIAN/postinst"
cp packaging/debian/postrm "$BUILD_DIR/DEBIAN/postrm"
chmod 755 "$BUILD_DIR/DEBIAN/postinst" "$BUILD_DIR/DEBIAN/postrm"

echo "== Building .deb =="
dpkg-deb --build --root-owner-group "$BUILD_DIR" "$DEB_FILE"

echo ""
echo "== Done =="
echo "Package: $(pwd)/${DEB_FILE}"
echo "Install with: sudo dpkg -i ${DEB_FILE}"
echo "Inspect with: dpkg-deb --info ${DEB_FILE} && dpkg-deb --contents ${DEB_FILE}"
