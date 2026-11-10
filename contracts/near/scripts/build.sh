#!/usr/bin/env bash
set -euo pipefail

near_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$near_root"
source "$near_root/scripts/toolchain.sh"

# Keep the required SDK host path even if the caller supplies encoded flags.
export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS:+${CARGO_ENCODED_RUSTFLAGS}$'\x1f'}--cfg"$'\x1f'"near"

mkdir -p artifacts

cargo build --locked --release --target wasm32-unknown-unknown \
    -p apodect-near-project

near_target="${CARGO_TARGET_DIR:-$near_root/target}"
cp "$near_target/wasm32-unknown-unknown/release/apodect_near_project.wasm" artifacts/

cargo build --locked --release --target wasm32-unknown-unknown \
    -p apodect-near-workspace \
    -p apodect-near-test-ft

cp "$near_target/wasm32-unknown-unknown/release/apodect_near_workspace.wasm" artifacts/
cp "$near_target/wasm32-unknown-unknown/release/apodect_near_test_ft.wasm" artifacts/
