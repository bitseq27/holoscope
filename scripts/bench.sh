#!/usr/bin/env bash
# M3 bench harness (plan §12/App. D): scripted flythrough, CPU+GPU frame stats.
# NOTE: p99 asserts are meaningful only on the reference GPU; on lavapipe/
# llvmpipe this is a functional smoke + relative regression signal.
set -euo pipefail
cd "$(dirname "$0")/.."
source /opt/hs-tools/env.sh 2>/dev/null || bash scripts/bootstrap.sh >/dev/null
cargo build -j "${HS_JOBS:-1}" 2>&1 | tail -1
exec /opt/hs-tools/target/debug/holoscope --headless \
    --place "${BENCH_PLACE:-Kyoto, Japan}" --extent "${BENCH_EXTENT:-4}" --vectors \
    --width "${BENCH_W:-1280}" --height "${BENCH_H:-720}" --points 1500000 \
    --bench "${1:-bench/fly_kyoto.toml}"
