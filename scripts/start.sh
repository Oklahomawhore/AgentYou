#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --locked -p yourself-server
exec python3 scripts/service.py start
