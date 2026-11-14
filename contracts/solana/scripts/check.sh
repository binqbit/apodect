#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

solana_build_root="${CARGO_TARGET_DIR:-$PWD/target}"
mkdir -p "$solana_build_root"
solana_build_root="$(cd "$solana_build_root" && pwd)"

cargo fmt -- --check

CARGO_TARGET_DIR="$solana_build_root/sbf" \
    cargo build-sbf --tools-version v1.53 --sbf-out-dir "$solana_build_root/deploy" -- --locked

CARGO_TARGET_DIR="$solana_build_root/host" \
    APODECT_SOLANA_PROGRAM="$solana_build_root/deploy/apodect_solana.so" \
    cargo test --locked
