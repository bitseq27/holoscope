#!/usr/bin/env bash
# M6 soak harness (plan §14): idle-orbit frames with RSS/GPU sampling.
# Real 8 h soaks run on the reference GPU; sandbox runs short smoke soaks.
# Usage: SOAK_MIN=0.5 scripts/soak.sh   -> bench/soak.csv + summary line
set -euo pipefail
cd "$(dirname "$0")/.."
source /opt/hs-tools/env.sh 2>/dev/null || bash scripts/bootstrap.sh >/dev/null
cargo build -j "${HS_JOBS:-1}" 2>&1 | tail -1
exec /opt/hs-tools/target/debug/holoscope --headless \
    --place "${SOAK_PLACE:-Kyoto, Japan}" --extent "${SOAK_EXTENT:-4}" --vectors \
    --width "${SOAK_W:-640}" --height "${SOAK_H:-480}" --points 800000 \
    --soak "${SOAK_MIN:-0.5}"
