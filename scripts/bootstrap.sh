#!/usr/bin/env bash
# HoloScope environment bootstrap — idempotent. Safe to re-run every session.
#
# Installs system build deps + software Vulkan (lavapipe) + Rust toolchain.
# NOTE: the toolchain & cargo caches live OUTSIDE the workspace (default
# /opt/hs-tools, override with HS_TOOLROOT): a multi-GB toolchain inside
# /home/user blows the workspace snapshot cap and the whole session's
# workspace delta gets dropped (learned the hard way — BUILD_LOG session 1).
set -euo pipefail

TOOLROOT=${HS_TOOLROOT:-/opt/hs-tools}
mkdir -p "$TOOLROOT"
export RUSTUP_HOME="$TOOLROOT/rustup"
export CARGO_HOME="$TOOLROOT/cargo"
export CARGO_TARGET_DIR="$TOOLROOT/target"
export PATH="$CARGO_HOME/bin:$PATH"

echo "== [1/3] apt packages =="
export DEBIAN_FRONTEND=noninteractive
PKGS=(build-essential pkg-config curl libvulkan-dev mesa-vulkan-drivers libwayland-dev libxkbcommon-dev libssl-dev ca-certificates)
missing=()
for p in "${PKGS[@]}"; do dpkg -s "$p" >/dev/null 2>&1 || missing+=("$p"); done
if [ ${#missing[@]} -gt 0 ]; then
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends "${missing[@]}"
else
  echo "all apt packages present"
fi

echo "== [2/3] rust toolchain =="
if ! rustc --version >/dev/null 2>&1; then
  curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
  sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain stable --no-modify-path
fi
rustc --version
cargo --version

echo "== [3/3] env file =="
ENVF="$TOOLROOT/env.sh"
{
  echo "export RUSTUP_HOME=$RUSTUP_HOME"
  echo "export CARGO_HOME=$CARGO_HOME"
  echo "export CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
  echo "export PATH=$CARGO_HOME/bin:\$PATH"
} > "$ENVF"
echo "bootstrap OK (source $ENVF)"
