# Apodect contracts

Apodect coordinates a Project budget across Ethereum, Solana and NEAR. The NEAR
coordinator owns approval ordering; each chain keeps its own token custody and
executes its agents' withdrawals and returns. Funds become spendable only after
the full approved principal is available on the task's chain.

The [contract rules](../docs/contracts.md) explain roles, accounting and lifecycle
behavior. The [coordination protocol](../docs/cross-chain-protocol.md) defines how
commands and actual custody evidence connect the chains.

## Develop locally

Install the tools in the [Ethereum](ethereum/README.md),
[Solana](solana/README.md) and [NEAR](near/README.md) setup guides, then run from
the repository root:

```sh
make local-up
make local-check
make local-smoke
make local-down
```

This is one persistent three-chain environment with one Project per chain and
local MPC/liquidity simulators. Stopping preserves state. To discard its ledgers,
service history and disposable keys, run `make local-reset CONFIRM=1`.
The [local guide](../docs/local-development.md) covers prerequisites and recovery;
the [workflow guide](../docs/cross-chain-development.md) shows how to use
`./scripts/localnet.sh` for tasks, funding and owner/authority actions.

## Check changes

Run `make check-ethereum`, `make check-solana` or `make check-near` for the package
you changed. These checks build contracts and exercise their chain runtimes without
requiring the persistent stack. `make check` also checks shared services and
orchestration; `make fmt` applies the maintained formatters.

Use integer token base units and the exact configured token identity. Native gas,
rent and storage are funded separately from Project USDC. Changed deployment
artifacts require a fresh local deployment; there is no automatic upgrade.

Local contract execution uses real native nodes and test tokens. External MPC
and delivery are simulated; public integration requirements are described in
[provider boundaries](../docs/cross-chain-providers.md).
