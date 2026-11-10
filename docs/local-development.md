# Local blockchain environments

Apodect provides three persistent development chains: Anvil for Ethereum, a Solana test validator, and NEAR sandbox. Each runs the actual compiled contracts and has an independent funded Project, a six-decimal test token, and separate owner, authority and agent identities. These environments implement the [local contract milestone](contracts.md); they do not synchronize budgets between networks.

## Requirements and first launch

Use Linux x86-64 with Bash, Make, Python 3.10+, Git and the native tools listed below. Initial builds download pinned Rust dependencies, contract libraries, compilers and the NEAR sandbox binary; allow disk space and time for native builds. Running all three nodes also needs enough resources for the Solana validator. Each network can be used independently.

| Network | Prerequisites | Package instructions |
| --- | --- | --- |
| Ethereum | Foundry 1.7.1 with `forge`, `cast`, `anvil`, and Solidity 0.8.33; alternatively Nix with flakes enabled, using the pinned automatic fallback | [Ethereum](../contracts/ethereum/README.md) |
| Solana | Solana CLI/test-validator 4.0.3, `cargo-build-sbf` 4.0.0 with platform-tools v1.53, host Rust 1.97.1+, `curl`, RootlessKit and slirp4netns; the last two have a pinned Nix fallback | [Solana](../contracts/solana/README.md) |
| NEAR | Rustup with Rust 1.99.0, rustfmt and the `wasm32-unknown-unknown` target, plus native Rust build tools; sandbox 2.14.0 is installed automatically | [NEAR](../contracts/near/README.md) |

From the repository root:

```sh
make local-up
make local-status
make local-check
make local-smoke
```

`local-up` builds the required artifacts, starts nodes on loopback, waits for RPC readiness, deploys contracts and creates funded actors on the first launch. Subsequent launches reuse the same chain and deployment. Nodes continue running after the command exits. `local-check` verifies the deployment through RPC, including contract code, configured token and roles. `local-smoke` sends signed transactions through RPC and verifies the local task lifecycle; it creates new task records on each run.

Select one network for any command:

```sh
make local-up CHAIN=ethereum
make local-smoke CHAIN=solana
make local-check CHAIN=near
```

These are wrappers around `python3 scripts/localnet.py <command> [all|ethereum|solana|near]`. The default is `all`. Chains are processed independently: if one fails, the command reports a failure and still attempts the others. Successfully started environments remain available.

## Endpoints, accounts and state

| Network | HTTP RPC | Additional ports |
| --- | --- | --- |
| Ethereum | `http://127.0.0.1:8545` | Chain ID `31337` |
| Solana | `http://127.0.0.1:8899` | WebSocket `8900`, faucet `9900` |
| NEAR | `http://127.0.0.1:3030` | Node network `24567` |

Each environment writes `.localnet/<chain>/manifest.json`. Read this file for actual contract addresses, token identity, RPC URL, actor addresses and paths to disposable signing keys. Contract addresses and account formats differ between networks; consumers should use their chain's manifest instead of hard-coded addresses. Amounts are integer token base units: `1_000_000` represents one six-decimal test token. Native gas, rent and storage balances are separate from Project token principal.

The complete generated environment is under the Git-ignored `.localnet/` directory. Node logs, keys, deployment manifests, process ownership records and chain databases belong to that directory. Directories and credentials are private to the local user. Source code and reusable tooling are under `scripts/` and `contracts/<chain>/`; they work without `.agent-workspace/`.

Owner, authority and agent are distinct local identities. Bootstrap deploys the supported local test asset, creates a Workspace/Project, registers or creates the necessary token accounts and funds the Project. Follow the package README for native transaction examples and chain-specific manifest fields. NEAR's contract-created Project has no signing key; callers act through its documented methods. Solana preloads the compiled program at its fixed development address when initializing the validator ledger.

## Stop, resume and reset

```sh
make local-down
make local-up
```

Stopping gracefully retains chain data, keys and deployment addresses. The manager records process start identity and signals only processes it launched. It refuses occupied ports and does not stop another application to obtain a port. Do not run two copies of a node against the same database.

To discard a selected development chain and all of its disposable keys:

```sh
make local-reset CHAIN=ethereum CONFIRM=1
make local-up CHAIN=ethereum
```

`make local-reset CONFIRM=1` resets all three. Reset without `CONFIRM=1` refuses deletion. Reset stops the selected owned node before removing its generated data. It does not remove source, dependencies or compiled contract artifacts. New bootstrap may produce different contract addresses and keys, so reload the manifest afterward.

When contract code changes, reset and rebuild the affected chain. Existing deployed contracts are not automatically upgraded. `local-check` rejects deployed code that differs from the current compiled artifacts. Use `local-down`, then `local-reset ... CONFIRM=1` and `local-up` for a clean deployment of changed code. Failed or ambiguous partial bootstrap is reported explicitly; reset the selected disposable environment when the adapter requires it.

## Port configuration and diagnostics

Set port overrides before the first launch and keep them set when restarting:

```sh
APODECT_ETHEREUM_RPC_PORT=18545 make local-up CHAIN=ethereum
APODECT_SOLANA_RPC_PORT=18899 APODECT_SOLANA_FAUCET_PORT=19900 make local-up CHAIN=solana
APODECT_NEAR_RPC_PORT=13030 APODECT_NEAR_NETWORK_PORT=34567 make local-up CHAIN=near
```

Solana's WebSocket port is always RPC port plus one. Configuration is retained in `.localnet/<chain>/settings.json`; checks, status and shutdown use that saved configuration. Changing a persistent environment's ports requires an explicit reset. All adapters require local loopback RPC and validate their own chain/deployment identity before sending transactions.

Agave's test-validator binds some services to wildcard addresses despite `--bind-address`. The Solana adapter therefore runs the native validator in a RootlessKit network namespace and publishes only the three loopback TCP ports above. Docker is not required. Linux must permit unprivileged user/network namespaces and have subordinate UID/GID ranges and the `newuidmap`/`newgidmap` helpers configured for your user. Once those host prerequisites are configured, running the environments requires no administrator access. `unshare --user --map-root-user --net true` checks basic namespace availability; RootlessKit also needs the ID mapping prerequisites. Startup fails explicitly if isolation is unavailable. The validator and RootlessKit use Unix sockets, so very long checkout paths must be shortened as directed by the adapter's path-length error.

```sh
make local-logs CHAIN=near
make local-status
```

`local-logs` shows the last 60 lines of each selected node log. For continuous output, use `tail -f .localnet/<chain>/node.log`. Startup/build/bootstrap failures are reported by the command; runtime node output is in that log. A failed startup stops its newly launched node and retains data for diagnosis. If graceful shutdown times out, the manager preserves the process record and database instead of forcing a kill.

Only one lifecycle command mutates a checkout's environments at a time. The manager uses an operating-system file lock, released when the command exits. If another operation is still running, let it finish before issuing another control command. Status and logs remain available while an operation is running.

## Verification

```sh
make check-localnet  # Process ownership, persistence, port and manifest guard tests.
make check          # Contract formatting, builds and maintained contract test suites.
make local-smoke    # Signed transactions against the running persistent RPC nodes.
```

The lifecycle-manager tests use an isolated HTTP process to exercise orchestration. The package tests use Foundry, compiled SBF in LiteSVM, and Wasm in NEAR sandbox. RPC smoke tests additionally verify the deployment and actual transactions on the persistent local nodes. None of these tests establishes public-chain deployment or cross-chain synchronization.

Verified on November 10, 2026: all three default-port environments deployed successfully, passed signed RPC lifecycle smoke tests, and retained deployment/state after graceful stop and restart. Host listener inspection confirmed loopback exposure. The regression run passed 42 Ethereum tests, 24 Solana tests, 18 NEAR unit tests plus six sandbox scenarios, and 10 lifecycle-manager tests.
