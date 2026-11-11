#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
export PYTHONDONTWRITEBYTECODE=1
umask 077
if [[ "${APODECT_CONTAINER:-0}" == 1 ]]; then
    exec /opt/apodect/venv/bin/python "$project_root/scripts/crosschain/main.py" "$@"
fi
export UV_PROJECT_ENVIRONMENT="$project_root/.localnet/tools/crosschain"
exec uv run --project "$project_root/scripts/crosschain" --locked --no-dev python \
    "$project_root/scripts/crosschain/main.py" "$@"
