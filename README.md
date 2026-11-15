# Apodect

One shared USDC project budget for independent agents across NEAR, Ethereum and Solana.

The owner supplies funds, a manager approves requests and starts funding, and agents
withdraw their confirmed allocations to do the work. A NEAR contract coordinates
the funding queue and reservations; contracts on each network hold the funds.
Committed and in-transit money cannot be allocated twice. Returned funds can
support the next approved task on another network.

## Run locally

Start with the [local setup and first task](docs/application-development.md#local-startup):
prerequisites, database secrets, test wallets and a complete request-to-return flow.
Use an x86-64 Linux Docker host with `io_uring` enabled and Compose 2.24.4+.
After creating the secrets, start the stack:

```sh
docker compose up --build --wait
```

Open <http://localhost:3000>. Compose starts the web app, Rust backend and worker,
PostgreSQL, Kafka, and all three local blockchains with deployed contracts. Local
MPC and liquidity simulators support the cross-chain flow. A project, test wallets
and initial liquidity are created automatically; no public RPC accounts or real
funds are needed. Public-provider execution requires separate
[integration validation](docs/cross-chain-providers.md).

## Read next

- [Product overview](docs/concept.md): the problem, roles and budget boundaries.
- [How it works](docs/how-it-works.md): signing in, funding tasks, returns and owner controls.
- [Architecture](docs/architecture.md): contracts, application services and synchronization.
- [Documentation index](docs/README.md): development, operations and protocol references.

The web app manages registered deployments; it does not deploy new projects.
Agents operate their own wallets. Apodect limits withdrawals from project vaults,
but cannot automatically recover funds already withdrawn to an independent wallet.

## License

Apodect is licensed under the [MIT License](LICENSE). Third-party components retain
their own licenses, including the [Moby seccomp profile](infra/localnet-seccomp.LICENSE).
