#!/usr/bin/env bash
#
# Generates a self-signed TLS certificate for local development/testing.
# Do NOT use this in production - use a certificate from a real CA
# (e.g. Let's Encrypt) for any publicly-reachable deployment.
#
# Usage: ./scripts/generate-dev-cert.sh

set -euo pipefail

mkdir -p certs

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout certs/dev-key.pem \
  -out certs/dev-cert.pem \
  -days 365 \
  -subj "/CN=localhost"

echo ""
echo "Generated:"
echo "  certs/dev-cert.pem"
echo "  certs/dev-key.pem"
echo ""
echo "Enable TLS in configs/nwarp.conf:"
echo "  tls_enabled = true"
echo "  tls_port = 9443"
echo "  tls_cert = ./certs/dev-cert.pem"
echo "  tls_key = ./certs/dev-key.pem"
