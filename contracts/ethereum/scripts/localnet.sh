#!/usr/bin/env bash
set -euo pipefail
umask 077

package_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v forge >/dev/null 2>&1 || ! command -v cast >/dev/null 2>&1 \
    || ! command -v anvil >/dev/null 2>&1; then
    if command -v nix >/dev/null 2>&1; then
        nixpkgs_ref='github:NixOS/nixpkgs/7dcdd87373d72e7a2173e4db02e20d3d7f495cfd'
        exec nix shell "$nixpkgs_ref#foundry" "$nixpkgs_ref#solc" \
            --command bash "$package_dir/scripts/localnet.sh" "$@"
    fi
    printf '%s\n' 'Foundry (forge, cast, anvil) or Nix is required.' >&2
    exit 1
fi

cd "$package_dir"
export FOUNDRY_ROOT="$package_dir"
if [[ -z "${FOUNDRY_SOLC:-}" ]] && command -v solc >/dev/null 2>&1; then
    if [[ "$(solc --version)" == *'Version: 0.8.33+'* ]]; then
        export FOUNDRY_SOLC="$(command -v solc)"
    fi
fi

exec python3 "$package_dir/scripts/localnet.py" "$@"
