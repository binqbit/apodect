# Documentation

## Understand and use Apodect

- [Product overview](concept.md): shared budgets, roles and control boundaries.
- [How it works](how-it-works.md): wallet access, requests, funding, returns and recovery.
- [Architecture](architecture.md): state ownership, application services and synchronization.

## Run and develop

- [Local setup and first task](application-development.md#local-startup): start Compose, sign in as each role and complete a task.
- [Live host](application-development.md#live-host): public deployment configuration, secrets and backups.
- [Backend services](backend.md): profiles, browser API, operational CLI, workers and configuration.
- [Native contract development](local-development.md): host prerequisites, checks and chain lifecycle.
- [Cross-chain workflows](cross-chain-development.md): native harness commands and fault scenarios.

Use Compose for the complete application. Use the native harness for contract
workflows; stop one before starting the other because they use the same RPC ports.

## Contract and integration reference

- [Contract rules](contracts.md): custody, shared/standalone allocation and lifecycle conditions.
- [Contract packages](../contracts/README.md): chain-specific builds and constraints.
- [Coordination protocol](cross-chain-protocol.md): deployment identity, commands, evidence and recovery.
- [Provider integration](cross-chain-providers.md): local simulators and public deployment requirements.

The runnable stack executes real local contracts and test-token transfers. External
MPC and liquidity providers are simulated; public execution requires separate validation.
