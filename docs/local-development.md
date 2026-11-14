# Native contract development

The host launcher runs Ethereum (Anvil), Solana test-validator and NEAR sandbox,
deploys contracts, and starts the Python relayer and simulated MPC/liquidity
providers. Contract calls and test-token transfers execute on the native nodes.
For the web/API stack, use [Docker Compose](application-development.md) instead.
Stop one launcher before starting the other: both use the same RPC ports.

## Requirements

Use Linux x86-64 with Bash, Make, Git, curl, Python 3.11+ and `uv`. Install the
pinned tools described by each package:

- [Ethereum](../contracts/ethereum/README.md): Foundry and Solidity.
- [Solana](../contracts/solana/README.md): CLI/validator, SBF build tools and host Rust.
- [NEAR](../contracts/near/README.md): Rustup toolchain, WASM target and native build dependencies.

Scripts fetch locked dependencies and NEAR sandbox on first use. Foundry and
RootlessKit have pinned Nix fallbacks. `uv` uses `.localnet/tools/` rather than
system Python. Initial builds and validator ledgers need substantial disk space.

Native Solana needs RootlessKit/slirp4netns, unprivileged user/network namespaces,
subordinate UID/GID ranges and `newuidmap`/`newgidmap`. Some validator listeners
ignore its bind address, so the launcher publishes only loopback RPC, WebSocket
and faucet ports through this namespace. Runtime does not require Docker or root.
Use a short checkout path if native Unix socket limits prevent startup.

## Start and inspect

```sh
make local-up
make local-status
make local-check
```

`local-up` builds/deploys one Project per chain, binds them to the NEAR coordinator,
initializes test actors/assets and synchronizes balances. Repeating it reuses the
same environment. `local-check` compares finalized vault custody with coordinator
accounting. Use [local workflows](cross-chain-development.md) for requests, funding,
agent actions and fault injection.

| Component | Default endpoint | Data directory |
| --- | --- | --- |
| Ethereum | `http://127.0.0.1:8545` | `.localnet/ethereum/` |
| Solana | `http://127.0.0.1:8899` | `.localnet/solana/` |
| NEAR | `http://127.0.0.1:3030` | `.localnet/near/` |
| Simulated provider API | `http://127.0.0.1:18080` | `.localnet/coordination/` |

Solana also uses WebSocket 8900 and faucet 9900; NEAR uses network port 24567.
The coordination manifest records deployed identities and endpoints; per-chain
manifests contain actors. Keys, manifests, journals and logs are Git-ignored.
Native gas, account rent and storage are separate from Project USDC.

Override a port before first start and retain that setting on restart, for example
`APODECT_ETHEREUM_RPC_PORT=18545 make local-up`. Supported settings are
`APODECT_<CHAIN>_<RPC|FAUCET|NETWORK>_PORT` for that chain's listed listeners and
`APODECT_PROVIDER_PORT`; Solana WebSocket follows its RPC port plus one.

## Stop and reset

```sh
make local-down
make local-up
make local-logs
```

Stopping preserves ledgers and journals and waits for in-flight submissions.
Status reports services, unsettled operations and errors; full logs remain in
chain/service data directories. Failed startup preserves diagnostic state.

To discard test chains, deployments, journals and disposable keys:

```sh
make local-reset CONFIRM=1
make local-up
```

Reset keeps source, toolchains and build caches. Contract changes need fresh
deployments; restarting does not upgrade existing contracts. Ledgers grow until
reset because settlement evidence must remain available. Missing historical
receipts do not establish that a payment failed.

## Checks

`make check` runs application, contract, adapter and service checks. Use
`make check-ethereum`, `make check-solana`, `make check-near`,
`make check-crosschain` or `make check-localnet` for focused changes.
`make fmt` formats maintained sources.

`make local-smoke` exercises the running native stack, including all three routes,
partial delivery, refunds, false provider success, restart recovery and agent
withdrawal/return/completion. It creates history and spends test transport fees;
run it without unfinished plans or unrelated activity.
