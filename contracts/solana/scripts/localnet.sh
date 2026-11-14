#!/usr/bin/env bash
set -euo pipefail
umask 077

cd "$(dirname "$0")/.."

solana_local_dir="${APODECT_LOCALNET_DIR:?APODECT_LOCALNET_DIR must be an absolute chain data directory}"
[[ "$solana_local_dir" = /* ]] || { echo "APODECT_LOCALNET_DIR must be absolute" >&2; exit 1; }
solana_rpc_port="${APODECT_RPC_PORT:-8899}"
solana_faucet_port="${APODECT_FAUCET_PORT:-9900}"
for solana_port in "$solana_rpc_port" "$solana_faucet_port"; do
    [[ "$solana_port" =~ ^[1-9][0-9]{3,4}$ ]] && ((solana_port >= 1024 && solana_port <= 65534)) || {
        echo "Solana local ports must be integers between 1024 and 65534" >&2
        exit 1
    }
done
[[ "$solana_faucet_port" != "$solana_rpc_port" && "$solana_faucet_port" -ne "$((solana_rpc_port + 1))" ]] || {
    echo "Solana RPC, websocket and faucet ports must be distinct" >&2
    exit 1
}
export APODECT_RPC_URL="${APODECT_RPC_URL:-http://127.0.0.1:$solana_rpc_port}"
[[ "$APODECT_RPC_URL" = "http://127.0.0.1:$solana_rpc_port" ]] || {
    echo "Solana localnet only accepts its configured loopback RPC URL" >&2
    exit 1
}
export APODECT_LOCALNET_DIR="$solana_local_dir"
export APODECT_SOLANA_PROGRAM="$solana_local_dir/program.so"

[[ "$(uname -s)" = Linux ]] || { echo "Solana localnet isolation requires Linux user/network namespaces" >&2; exit 1; }
solana_isolation=()
if [[ "${APODECT_CONTAINER:-0}" != 1 ]]; then
    solana_isolation=(rootlesskit)
    if ! command -v rootlesskit >/dev/null || ! command -v slirp4netns >/dev/null; then
        command -v nix >/dev/null || { echo "Install rootlesskit and slirp4netns, or Nix, for loopback-only Solana networking" >&2; exit 1; }
        solana_nixpkgs="github:NixOS/nixpkgs/7dcdd87373d72e7a2173e4db02e20d3d7f495cfd"
        solana_isolation=(nix shell "$solana_nixpkgs#rootlesskit" "$solana_nixpkgs#slirp4netns" --command rootlesskit)
    fi
fi

case "${1:-}" in
    prepare)
        mkdir -p "$solana_local_dir/bin"
        if [[ "${APODECT_CONTAINER:-0}" == 1 ]]; then
            cp /opt/apodect/apodect_solana.so "$APODECT_SOLANA_PROGRAM"
            cp /opt/apodect/bin/solana-client "$solana_local_dir/bin/client"
            exec "$solana_local_dir/bin/client" prepare
        fi
        for solana_tool in cargo cargo-build-sbf solana-test-validator curl; do
            command -v "$solana_tool" >/dev/null || { echo "Missing prerequisite: $solana_tool" >&2; exit 1; }
        done
        "${solana_isolation[@]}" --version
        solana_build_root="${CARGO_TARGET_DIR:-$PWD/target}"
        mkdir -p "$solana_build_root"
        solana_build_root="$(cd "$solana_build_root" && pwd)"
        CARGO_TARGET_DIR="$solana_build_root/sbf" \
            cargo build-sbf --tools-version v1.53 --sbf-out-dir "$solana_local_dir/build" -- --locked
        CARGO_TARGET_DIR="$solana_build_root/host" \
            cargo build --locked --features no-entrypoint --example localnet
        cp "$solana_local_dir/build/apodect_solana.so" "$APODECT_SOLANA_PROGRAM"
        cp "$solana_build_root/host/debug/examples/localnet" "$solana_local_dir/bin/client"
        "$solana_local_dir/bin/client" prepare
        ;;
    serve)
        # Keep native receipt/block proofs for the full lifetime of this local ledger.
        # The validator defaults to pruning after 10,000 shreds, even if omitted.
        export RUST_LOG="${RUST_LOG:-warn}"
        # Gossip needs a concrete address; Agave binds HTTP/WebSocket RPC to 0.0.0.0 separately.
        solana_validator=(
            solana-test-validator
            --config "$solana_local_dir/cli.yml"
            --ledger "$solana_local_dir/ledger"
            --limit-ledger-size 18446744073709551615
            --bind-address 127.0.0.1
            --rpc-port "$solana_rpc_port"
            --faucet-port "$solana_faucet_port"
            --bpf-program 9HLdesoinPvKWr56HRfpbGL9M8yKWzzBFNAr4rbZLkQP "$APODECT_SOLANA_PROGRAM"
            --log
        )
        if [[ "${APODECT_CONTAINER:-0}" == 1 ]]; then
            # Docker supplies the network/PID isolation; no nested user namespace.
            exec "${solana_validator[@]}"
        fi
        exec "${solana_isolation[@]}" \
            --state-dir "$solana_local_dir/rootlesskit" \
            --net=slirp4netns --copy-up=/etc --disable-host-loopback \
            --port-driver=builtin --pidns --reaper=true \
            --publish "127.0.0.1:$solana_rpc_port:$solana_rpc_port/tcp4" \
            --publish "127.0.0.1:$((solana_rpc_port + 1)):$((solana_rpc_port + 1))/tcp4" \
            --publish "127.0.0.1:$solana_faucet_port:$solana_faucet_port/tcp4" \
            "${solana_validator[@]}"
        ;;
    bootstrap|check|bridge)
        exec "$solana_local_dir/bin/client" "$1"
        ;;
    *)
        echo "Usage: $0 {prepare|serve|bootstrap|check|bridge}" >&2
        exit 2
        ;;
esac
