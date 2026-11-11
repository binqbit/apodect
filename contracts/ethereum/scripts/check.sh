#!/usr/bin/env bash
set -euo pipefail

package_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v forge >/dev/null 2>&1; then
    if command -v nix >/dev/null 2>&1; then
        nixpkgs_ref='github:NixOS/nixpkgs/7dcdd87373d72e7a2173e4db02e20d3d7f495cfd'
        exec nix shell "$nixpkgs_ref#foundry" "$nixpkgs_ref#solc" \
            --command bash "$package_dir/scripts/check.sh" "$@"
    fi
    printf '%s\n' 'Foundry is required. Install it from https://getfoundry.sh/ or provide Nix.' >&2
    exit 1
fi

cd "$package_dir"
export FOUNDRY_ROOT="$package_dir"

if [[ -z "${FOUNDRY_SOLC:-}" ]] && command -v solc >/dev/null 2>&1; then
    if [[ "$(solc --version)" == *'Version: 0.8.33+'* ]]; then
        export FOUNDRY_SOLC="$(command -v solc)"
    fi
fi

PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py' -v

# Soldeer uses the exact revisions recorded in the package configuration and lockfile.
forge soldeer install --config-location soldeer
forge fmt --check
forge build --sizes --skip test --skip script

# Enforce production limits independently of test VM settings. Forge's size table
# omits constructor arguments; these fixed constructors are all static ABI words.
python3 - <<'PYTHON'
import json
from pathlib import Path

constructors = {"ProjectVault": 96, "WorkspaceRegistry": 32, "RouteEscrow": 32}
for name, constructor_bytes in constructors.items():
    artifact = json.loads(Path(f"out/{name}.sol/{name}.json").read_text())
    runtime = len(bytes.fromhex(artifact["deployedBytecode"]["object"][2:]))
    initcode = len(bytes.fromhex(artifact["bytecode"]["object"][2:])) + constructor_bytes
    if runtime > 24_576 or initcode > 49_152:
        raise SystemExit(
            f"{name} exceeds EVM deployment limits: runtime={runtime}, initcode={initcode}"
        )
    print(
        f"{name} deployment limits passed: runtime={runtime}/24576, "
        f"initcode={initcode}/49152 bytes"
    )
PYTHON

forge test "$@"
