# Apodect

Project budget coordination for independent partner agents across networks.

A team can know its overall Project budget without knowing in advance how much each agent or network will need. Apodect connects those requests through NEAR coordination and local contract-controlled Project funds on NEAR, Ethereum, and Solana.

The owner allocates each Project's budget and appoints authority. Authority approves agents' requests and starts the funding plan. Agents receive full approved allocations in priority order; committed funds and transfers in progress are not available for a second allocation.

Partners operate their own agents. Once funds leave controlled vaults for an independent wallet, Apodect cannot automatically freeze or recover them. Agents return remaining funds before completing a task, and the Project records returns, task expenses, and delivery fees separately.

Read the [Concept](docs/concept.md), [Architecture](docs/architecture.md), and [How It Works](docs/how-it-works.md), or start at the [documentation index](docs/README.md).

The [contracts](docs/contracts.md) support independent Project lifecycles and an explicitly bound coordinated mode. A NEAR coordinator manages the shared approval queue, reservations and authenticated commands; each network enforces its own custody. The [cross-chain environment](docs/cross-chain-development.md) combines real local RPC transactions with separate local simulations of Chain Signatures, Read MPC and 1Click delivery. See [provider boundaries](docs/cross-chain-providers.md) before connecting public infrastructure.

Follow the [application guide](docs/application-development.md) to create database
secrets, then run `docker compose up --build --wait`. It starts React, Rust/Actix Web,
PostgreSQL, Kafka, a Rust worker and one localnet service containing all three
chains, deployed contracts and simulated providers. The worker indexes finalized
state, relays coordinator commands and reconciles liquidity. Wallet authentication
and unsigned user actions are available through the [backend services](docs/backend.md);
the business HTTP API and web interface remain for the next stage. The live
override excludes localnet.

For contract development directly on the host, `make local-up` remains available;
stop Compose first because both launchers use the same RPC ports. See
[native local development](docs/local-development.md) and
[workflows](docs/cross-chain-development.md) for checks, funding and agent lifecycles.

Run `make check` for contract, adapter and service checks, or `make fmt` to apply
the repository formatters. Public MPC and liquidity integration remain separate
from the local simulator; see [provider requirements](docs/cross-chain-providers.md).
