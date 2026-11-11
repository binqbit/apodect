# Local development

One environment runs Ethereum (Anvil), Solana test-validator and NEAR sandbox,
deploys the contracts, and starts the relayer and simulated MPC/liquidity providers.
All contract calls and test-token transfers execute on those native nodes.
External provider behavior is simulated; see [integration boundaries](cross-chain-providers.md).

## Requirements

Use Linux x86-64 with Bash, Make, Git, curl, Python 3.11+ and `uv`, plus:

| Package | Tools |
| --- | --- |
| [Ethereum](../contracts/ethereum/README.md) | Foundry 1.7.1 and Solidity 0.8.33 |
| [Solana](../contracts/solana/README.md) | CLI/test-validator 4.0.3, cargo-build-sbf 4.0.0 with platform-tools v1.53, host Rust 1.97.1+, RootlessKit and slirp4netns |
| [NEAR](../contracts/near/README.md) | Rustup toolchain 1.99.0 with rustfmt and wasm32-unknown-unknown; native build tools |

The scripts download pinned dependencies and NEAR sandbox 2.14.0 on first use.
Foundry and RootlessKit have pinned Nix fallbacks when unavailable locally.
`uv` installs locked Python dependencies under `.localnet/tools/`; it does not
change system Python. Initial builds and the Solana validator need substantial
disk space and memory.

Solana runs inside a RootlessKit network namespace because the validator exposes
some listeners beyond its configured bind address. Enable unprivileged user/network
namespaces and configure subordinate UID/GID ranges with `newuidmap`/`newgidmap`.
Only its RPC, WebSocket and faucet TCP ports are published on loopback. Docker and
administrator access are not needed at runtime. A very long checkout path may need
shortening to fit native Unix socket limits.

## Start and use

```sh
make local-up
make local-status
make local-check
make local-smoke
```

`local-up` builds, deploys and binds one Project on each chain to the NEAR
coordinator, initializes test assets and actors, starts services and synchronizes
balances. Repeating it reuses the existing environment. `local-check` verifies
finalized custody against the coordinator. `local-smoke` exercises all three routes,
partial delivery, delayed refunds, false provider success, restart recovery and
agent withdrawals/returns. It creates task history and spends test transport fees.
Run it with no unfinished plans or unrelated activity.

Use [the workflow guide](cross-chain-development.md) to create tasks, approve
budgets, fund Projects and exercise failure recovery.

| Component | Default endpoint | Data |
| --- | --- | --- |
| Ethereum | `http://127.0.0.1:8545` | `.localnet/ethereum/` |
| Solana | `http://127.0.0.1:8899` | `.localnet/solana/` |
| NEAR | `http://127.0.0.1:3030` | `.localnet/near/` |
| Simulated provider API | `http://127.0.0.1:18080` | `.localnet/coordination/` |

Solana also uses WebSocket 8900 and faucet 9900; NEAR uses network port 24567.
The coordination manifest records the actual RPCs, vaults, tokens and domains.
Per-chain manifests contain deployment and actor details. All keys, databases,
manifests and logs are Git-ignored. Native gas, rent and storage are separate from
Project USDC.

Port overrides are `APODECT_<CHAIN>_<RPC|FAUCET|NETWORK>_PORT` and
`APODECT_PROVIDER_PORT`. Set them before the first start and retain them on restart.
For example, `APODECT_ETHEREUM_RPC_PORT=18545 make local-up` changes Ethereum's
port in this same environment. There is no second node profile.

## Stop, diagnose and reset

```sh
make local-down
make local-up
make local-logs
```

Stopping preserves state and waits for in-flight native submissions. Status shows
node/service health, unsettled payments and reconciliation errors. Logs include
nodes and services; full logs remain in their data directories. A failed startup
retains diagnostic data. The manager only signals owned processes and rejects
occupied ports. One lifecycle operation runs at a time.

To discard the local chains, deployments, service journals and disposable keys:

```sh
make local-reset CONFIRM=1
make local-up
```

Reset is destructive and explicit; it keeps source, toolchains and build caches.
Contract changes require a fresh deployment, not an automatic upgrade. Retain any
valuable history before resetting. Environments from the old separate-profile
setup are not migrated or deleted automatically; start fresh with this layout.

Local Ethereum finality advances with empty blocks; Solana and NEAR retain the
history needed to reverify receipts after restart. Their ledgers grow until reset.
A missing historical receipt never means that its payment failed.

## Development checks

`make check` runs formatting, builds and the contract, adapter and service suites.
Use `make check-ethereum`, `make check-solana`, `make check-near`,
`make check-crosschain` or `make check-localnet` for a focused change.
`make fmt` formats maintained Solidity, Rust and Python. `make local-smoke`
complements these checks with the actual running stack.
