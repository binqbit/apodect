# Ethereum contracts

Each Project has a separate vault contract created through the Workspace registry.
See the [contract rules](../../docs/contracts.md) for shared behavior and the
[vault source](src/ProjectVault.sol) for native calls.

## Build and check

Use Bash, Git, Python 3 and Foundry 1.7.1 with Soldeer support. Solidity is pinned
to 0.8.33. Install Foundry using its [official instructions](https://getfoundry.sh/introduction/installation/),
or provide Nix so the scripts can obtain the pinned Foundry/solc toolchain.
The first dependency installation needs network access.

From the repository root:

```sh
make check-ethereum
./contracts/ethereum/scripts/fmt.sh
```

The check installs locked Soldeer dependencies, checks formatting, enforces EVM
runtime/initcode size limits and runs tests. It needs no persistent node.
`FOUNDRY_SOLC` may select an existing compatible compiler.

## Integration limits

- Use the exact configured six-decimal, non-rebasing ERC-20. Fee-on-transfer
  tokens are unsupported; direct donations do not credit the Project ledger.
- Bindings are fixed, with no upgrade entrypoint or accounting override. Rebuild
  and redeploy changed artifacts; CREATE2 addresses depend on creation code.
- Reconcile finalized transactions after uncertain submissions. An included
  transaction alone does not establish finality.

The [local guide](../../docs/local-development.md) starts the shared environment;
the [provider guide](../../docs/cross-chain-providers.md) covers public integration.
