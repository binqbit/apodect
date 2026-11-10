# Apodect

Project budget coordination for independent partner agents across networks.

A team can know its overall Project budget without knowing in advance how much each agent or network will need. Apodect connects those requests through NEAR coordination and local contract-controlled Project funds on NEAR, Ethereum, and Solana.

The owner allocates each Project's budget and appoints authority. Authority approves agents' requests and starts the funding plan. Agents receive full approved allocations in priority order; committed funds and transfers in progress are not available for a second allocation.

Partners operate their own agents. Once funds leave controlled vaults for an independent wallet, Apodect cannot automatically freeze or recover them. Agents return remaining funds before completing a task, and the Project records returns, task expenses, and delivery fees separately.

Read the [Concept](docs/concept.md), [Architecture](docs/architecture.md), and [How It Works](docs/how-it-works.md), or start at the [documentation index](docs/README.md).

The [local contracts milestone](docs/contracts.md) implements independent Project lifecycles on the three networks. See [contract packages and checks](contracts/README.md) for development. The complete cross-chain flow remains the intended product architecture; shared budget synchronization and external delivery/evidence integrations are deferred.

Run `make local-up` to start persistent Ethereum, Solana and NEAR development nodes with deployed contracts, test tokens and funded actors. Use `make local-smoke` to exercise them through RPC and `make local-down` to stop them while retaining state. See [local development](docs/local-development.md) for prerequisites, endpoints and reset commands.
