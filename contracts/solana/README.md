# Solana contracts

The Solana program keeps each Project's funds in its own PDA-controlled token
vault. It authenticates coordinator commands and preserves local agent control
over withdrawals and returns. Full funding and actual token receipts determine
spending rights; provider status alone cannot grant them. The shared
[contract rules](../../docs/contracts.md) and [protocol](../../docs/cross-chain-protocol.md)
explain the cross-chain behavior.

## Build and check

Use Solana CLI/test-validator 4.0.3, cargo-build-sbf 4.0.0 with platform-tools
v1.53, and host Rust 1.97.1 or newer. Anchor 1.2.1 and LiteSVM 0.18.2 are pinned
with their compatible Solana dependencies. Preserve the lockfile and upgrade
these tools together. The first build needs network access.

From the repository root:

```sh
make check-solana
./contracts/solana/scripts/fmt.sh
```

The check compiles SBF and executes it with SPL Token instructions in LiteSVM;
it needs no running validator or wallet. Scripts separate host and SBF outputs
under `CARGO_TARGET_DIR` so their Rust toolchains do not share incompatible builds.
Use the crate's generated Anchor builders and [program interface](src/lib.rs)
when constructing native calls.

## Use the local stack

`make local-up` starts all three chains, preloads the compiled Solana program and
binds one Project with 100,000 local test USDC. Use the common check, smoke,
shutdown and explicit reset commands in the [local guide](../../docs/local-development.md).
Task and funding commands use the shared `./scripts/localnet.sh` workflow.

The persistent validator requires Linux, `curl`, RootlessKit and slirp4netns, with
unprivileged user/network namespaces, subordinate UID/GID ranges and
`newuidmap`/`newgidmap`. Installed isolation tools are used when available; the
adapter also supports their pinned Nix packages. Some validator listeners ignore
the requested bind address, so the stack isolates the process and publishes only
loopback RPC, WebSocket and faucet ports. A short checkout/data path may be needed
for native Unix socket limits.

## Integration limits

Only the legacy SPL Token program and the exact configured six-decimal mint are
accepted. Amounts fit `u64`; transaction fees and account rent are separate from
USDC principal. Direct vault transfers remain outside the accounted balance.

Retain the validator ledger for funding and command evidence. Increasing retention
cannot restore a pruned transaction, and an unknown signature never authorizes a
fresh payment. Ledger disk use grows until an explicit local reset.

The development program address is for local preload. A public deployment needs
its own program address, authority policy, asset and provider configuration.
Changed program artifacts require a fresh local ledger; existing accounts are
not migrated automatically. See [provider boundaries](../../docs/cross-chain-providers.md)
before extending the local simulator setup to public services.
