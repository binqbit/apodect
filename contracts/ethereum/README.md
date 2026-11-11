# Ethereum contracts

The Ethereum vault keeps each Project's USDC separate and enforces its agents'
spending rights. The NEAR coordinator approves and orders work across chains;
signed commands can reserve and allocate this vault's funds without giving the
relayer arbitrary spending authority. See the [contract rules](../../docs/contracts.md)
and [coordination protocol](../../docs/cross-chain-protocol.md) for the shared behavior.

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

The check builds with the package's compiler settings and exercises token/accounting
behavior in the EVM. It also enforces normal deployment size limits, including
factory-embedded creation code. Keep these gates when changing dependencies or
optimizer settings. `FOUNDRY_SOLC` may select an existing compatible compiler.

## Use the local stack

`make local-up` starts the shared three-chain environment and binds its Ethereum
Project to the coordinator. This Project starts with zero USDC; fund it or route
liquidity to it through the [workflow guide](../../docs/cross-chain-development.md).
The fixture supplies local ETH for gas and a finite provider token inventory.
Use the common `make local-check`, `make local-smoke`, `make local-down` and
explicit reset commands from the [local guide](../../docs/local-development.md).

Anvil advances local finality with empty blocks. A restored snapshot can briefly
precede readable finalized state, so startup waits for finality. An included
transaction or an RPC timeout does not establish a settled financial effect;
recovery must reconcile the original signed transaction.

## Integration limits

Bind the intended six-decimal, exact-amount, non-rebasing ERC-20 by address.
Transfer fees and rebasing are unsupported. Direct token donations do not credit
the Project ledger, and there is no administrative rescue or accounting override.
Use attributed funding and return operations.

Vault bindings are fixed and the contracts expose no upgrade entrypoint. Rebuild
and explicitly redeploy changed artifacts; CREATE2 addresses depend on the exact
creation code. Use the [vault interface](src/ProjectVault.sol) for call details.
Local test USDC and simulated delivery/MPC services do not configure a public
provider deployment; consult the [provider boundaries](../../docs/cross-chain-providers.md).
