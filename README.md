# Apodect

Project budget coordination for independent partner agents across networks.

A team can know its overall Project budget without knowing in advance how much each agent or network will need. Apodect connects those requests through NEAR coordination and local contract-controlled Project funds on NEAR, Ethereum, and Solana.

The owner allocates each Project's budget and appoints authority. Authority approves agents' requests and starts the funding plan. Agents receive full approved allocations in priority order; committed funds and transfers in progress are not available for a second allocation.

Partners operate their own agents. Once funds leave controlled vaults for an independent wallet, Apodect cannot automatically freeze or recover them. Agents return remaining funds before completing a task, and the Project records returns, task expenses, and delivery fees separately.

Read the [Concept](docs/concept.md), [Architecture](docs/architecture.md), and [How It Works](docs/how-it-works.md), or start at the [documentation index](docs/README.md).

The [contracts](docs/contracts.md) support independent Project lifecycles and an explicitly bound coordinated mode. A NEAR coordinator manages the shared approval queue, reservations and authenticated commands; each network enforces its own custody. The [cross-chain environment](docs/cross-chain-development.md) combines real local RPC transactions with separate local simulations of Chain Signatures, Read MPC and 1Click delivery. See [provider boundaries](docs/cross-chain-providers.md) before connecting public infrastructure.

Run `make local-up` for the complete local stack: Ethereum, Solana, NEAR, deployed
contracts, relayer and simulated providers. `make local-smoke` exercises cross-chain
funding and agent lifecycles; `make local-down` stops the stack while retaining state.
See [local development](docs/local-development.md) for setup and
[workflows](docs/cross-chain-development.md) for task and funding commands.

Run `make check` for contract, adapter and service checks, or `make fmt` to apply
the repository formatters. Public MPC and liquidity integration remain separate
from the local simulator; see [provider requirements](docs/cross-chain-providers.md).
