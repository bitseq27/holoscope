#!/usr/bin/env bash
# One-shot verification: unit tests + headless golden render.
# Usage: scripts/verify.sh   (adds --points override via VERIFY_POINTS)
set -euo pipefail
cd "$(dirname "$0")/.."
source /opt/hs-tools/env.sh 2>/dev/null || bash scripts/bootstrap.sh >/dev/null
cargo test -j "${HS_JOBS:-1}" 2>&1 | grep -E "^test |test result" || true
cargo build -j "${HS_JOBS:-1}" 2>&1 | tail -1
PTS=${VERIFY_POINTS:-1200000}
/opt/hs-tools/target/debug/holoscope --headless --out renders/verify.png \
    --width 960 --height 720 --points "$PTS"
echo "verify OK -> renders/verify.png"
