#!/usr/bin/env bash
set -euo pipefail

package_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
if ! command -v forge >/dev/null 2>&1; then
    if command -v nix >/dev/null 2>&1; then
        nixpkgs_ref='github:NixOS/nixpkgs/7dcdd87373d72e7a2173e4db02e20d3d7f495cfd'
        exec nix shell "$nixpkgs_ref#foundry" \
            --command bash "$package_dir/scripts/fmt.sh" "$@"
    fi
    printf '%s\n' 'Foundry is required. Install it from https://getfoundry.sh/ or provide Nix.' >&2
    exit 1
fi

cd "$package_dir"
export FOUNDRY_ROOT="$package_dir"

exec forge fmt "$@"
