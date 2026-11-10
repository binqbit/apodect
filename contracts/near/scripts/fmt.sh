#!/usr/bin/env bash
set -euo pipefail

near_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$near_root"
source "$near_root/scripts/toolchain.sh"

cargo fmt --all
