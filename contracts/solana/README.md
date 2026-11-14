# Solana contracts

Each Project holds funds in its own PDA-controlled token vault. Instruction
handlers and their account contexts live together under `src/instructions/`;
shared records live in `src/state.rs`. See the [contract rules](../../docs/contracts.md)
for behavior and the [program entrypoints](src/lib.rs) for native calls.

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
it needs no running validator or wallet. Host and SBF outputs remain separate
under `CARGO_TARGET_DIR`.

## Integration limits

- Only the legacy SPL Token program and configured six-decimal mint are accepted.
  Amounts fit `u64`; native fees and rent are separate from USDC. Direct vault
  transfers do not credit the ledger.
- Retain transaction history and immutable receipts for unsettled operations.
  A pruned or unknown signature is not proof of failure.
- The development address is preloaded locally. Public deployment needs a chosen
  program address, matching program/client builds and an upgrade-authority policy.
  Changed artifacts require a fresh local ledger; accounts are not migrated.

The persistent validator uses RootlessKit to expose only loopback services.
See [native prerequisites](../../docs/local-development.md) for namespace setup
and [provider integration](../../docs/cross-chain-providers.md) for public deployment.
