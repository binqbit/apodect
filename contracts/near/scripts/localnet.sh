#!/usr/bin/env bash
set -euo pipefail
umask 077

near_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$near_root"
source "$near_root/scripts/toolchain.sh"
export APODECT_NEAR_ARTIFACT_DIR="$near_root/artifacts"

: "${APODECT_LOCALNET_DIR:?Set APODECT_LOCALNET_DIR to an absolute persistent data directory}"
[[ "$APODECT_LOCALNET_DIR" = /* ]] || { echo 'APODECT_LOCALNET_DIR must be absolute' >&2; exit 1; }
export APODECT_RPC_PORT="${APODECT_RPC_PORT:-3030}"
export APODECT_NETWORK_PORT="${APODECT_NETWORK_PORT:-24567}"
export APODECT_RPC_URL="${APODECT_RPC_URL:-http://127.0.0.1:$APODECT_RPC_PORT}"
for near_port in "$APODECT_RPC_PORT" "$APODECT_NETWORK_PORT"; do
    [[ "$near_port" =~ ^[1-9][0-9]{0,4}$ ]] && ((near_port <= 65535)) || {
        echo 'Local NEAR ports must be decimal integers from 1 to 65535' >&2
        exit 1
    }
done
[[ "$APODECT_RPC_URL" == "http://127.0.0.1:$APODECT_RPC_PORT" ]] || {
    echo 'APODECT_RPC_URL must exactly match the configured loopback RPC port' >&2
    exit 1
}

case "${1:-}" in
    prepare)
        mkdir -p "$APODECT_LOCALNET_DIR"
        ./scripts/build.sh
        cargo build --locked -p apodect-near-localnet
        near_target="${CARGO_TARGET_DIR:-$near_root/target}"
        near_target="$(cd "$near_target" && pwd)"
        printf '%s\n' "$near_target/debug/apodect-near-localnet" > "$APODECT_LOCALNET_DIR/client.path"
        "$near_target/debug/apodect-near-localnet" prepare
        ;;
    serve)
        near_binary="$(cat "$APODECT_LOCALNET_DIR/sandbox.path")"
        exec "$near_binary" --home "$APODECT_LOCALNET_DIR/node" run \
            --rpc-addr "127.0.0.1:$APODECT_RPC_PORT" \
            --network-addr "127.0.0.1:$APODECT_NETWORK_PORT"
        ;;
    bootstrap|check|bridge)
        near_client="$(cat "$APODECT_LOCALNET_DIR/client.path")"
        exec "$near_client" "$1"
        ;;
    *)
        echo 'Usage: localnet.sh {prepare|serve|bootstrap|check|bridge}' >&2
        exit 2
        ;;
esac
