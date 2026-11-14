# Apodect contracts

The NEAR coordinator controls a Project's shared funding queue; Ethereum, Solana
and NEAR vaults retain custody and enforce local agent allocations.

The [contract rules](../docs/contracts.md) explain roles, accounting and lifecycle
behavior. The [coordination protocol](../docs/cross-chain-protocol.md) defines how
commands and actual custody evidence connect the chains.

| Package | Toolchain, checks and integration constraints |
| --- | --- |
| [Ethereum](ethereum/README.md) | EVM registry, Project vaults and route escrow |
| [Solana](solana/README.md) | PDA-controlled vaults and instruction handlers |
| [NEAR](near/README.md) | Project factory, vaults, coordinator and route receivers |

Use [Compose](../docs/application-development.md) for the full application or
[native local development](../docs/local-development.md) for host contract work.
The [workflow guide](../docs/cross-chain-development.md) covers task and funding
commands. All commands run from the repository root.

`make check` checks contracts, adapters and applications; `make fmt` applies the
repository formatters. Changed contract artifacts need a fresh local deployment.
