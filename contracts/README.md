# Apodect contracts

Three implementations of the [local Project lifecycle](../docs/contracts.md), with separate custody and the same business rules:

| Package | Execution | Verification |
| --- | --- | --- |
| [Ethereum](ethereum/README.md) | Solidity, ERC-20 Project vaults | Foundry EVM tests |
| [Solana](solana/README.md) | Rust/Anchor, Project SPL Token vaults | Compiled SBF in LiteSVM |
| [NEAR](near/README.md) | Rust, separate NEP-141 Project accounts | Wasm in NEAR sandbox |

Install the prerequisites documented by each package. From the repository root:

```sh
make local-up
make local-smoke
make local-down
```

These commands manage persistent native development nodes, deployed contracts, local test assets and funded actors. Select one chain with `CHAIN=ethereum`, `CHAIN=solana` or `CHAIN=near`. See [local development](../docs/local-development.md) for installation, RPC endpoints, manifests, state persistence and reset instructions.

The independent contract test suites remain available:

```sh
make check-ethereum
make check-solana
make check-near
```

`make check` runs all three package checks. Each checks formatting, compiles its contracts, and runs its maintained local tests. `make fmt` applies the corresponding language formatter. Package scripts work independently of the private `.agent-workspace` directory. Dependency lockfiles belong in Git; generated dependencies, compiled artifacts, and test ledgers do not.

Use actual integer token base units. Configure the exact intended token identity; the tests deploy local test assets. Native gas, rent, and storage are funded separately from token principal. Test identities and assets are local and do not authorize public transactions.

The package READMEs document interfaces, account requirements, chain-specific asynchronous behavior, and toolchain versions. [The shared specification](../docs/contracts.md) defines roles, FIFO admission, accounting, return-before-finalize, and pause/restoration semantics across all three implementations.

[Source architecture](../docs/contract-architecture.md) explains module responsibilities, the review decisions, and compatibility checks for changes to the implementation structure.

The contracts in this milestone operate independent local budgets. Cross-chain synchronization, remote authorization, 1Click, Read MPC, backend/UI, and public deployment verification remain subsequent work.
