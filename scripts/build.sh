#!/usr/bin/env bash
# sandbox-friendly build (1 GiB RAM box): serial jobs avoid rustc OOM.
set -euo pipefail
source /opt/hs-tools/env.sh 2>/dev/null || bash "$(dirname "$0")/bootstrap.sh" >/dev/null
cd "$(dirname "$0")/.."
exec cargo build -j "${HS_JOBS:-1}" "$@"
