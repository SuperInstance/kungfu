#!/bin/sh
set -e

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
    aarch64) ARCH="arm64" ;;
esac

BINARY_NAME="kungfu-${OS}-${ARCH}"

cargo build --release
mkdir -p dist
cp target/release/kungfu "dist/${BINARY_NAME}"
cp target/release/kungfu dist/kungfu

echo "Built: dist/${BINARY_NAME}"
