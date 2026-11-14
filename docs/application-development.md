# Application development

One Compose command starts the React frontend, Rust/Actix Web backend,
PostgreSQL, Kafka, a Rust worker and the local blockchain environment. The
`localnet` service runs Ethereum, Solana and NEAR, deploys contracts, and starts
the simulated MPC/liquidity providers. The separate worker indexes contracts,
relays authorized commands and reconciles finalized receipts.

The web application includes wallet sign-in, linked profiles and project workspaces
for tasks, approvals, funding, invoices and recovery. The HTTP API prepares unsigned
actions; users sign in their own wallets. The operational CLI exposes the same
services; see [backend workflows](backend.md). Local blockchain transactions are
real; external provider behavior is simulated.

## Local startup

Run commands from the repository root. Use an x86-64 Linux Docker host with
`io_uring` enabled, Compose 2.24.4 or newer, and OpenSSL. The images include the
application and blockchain toolchains; the localnet image targets Linux/amd64.
The initial build can take considerable time and disk space because it compiles
contracts and native clients.
The localnet service uses Docker's standard seccomp rules with the three
`io_uring` calls allowed for Agave 4.

If the native stack is already running, stop it with `make local-down` first.
Native and Compose launchers use the same standard RPC ports and cannot run
together. Stopping either preserves its data.

Generate the two database passwords once. This command preserves existing files:

```sh
mkdir -p .secrets
chmod 0700 .secrets
(
  set -eu
  umask 077
  for name in postgres-admin postgres-app; do
    if [ ! -e ".secrets/$name" ]; then
      (set -C; openssl rand -hex 32 > ".secrets/$name")
    fi
    chmod 0444 ".secrets/$name"
  done
)
docker compose up --build --wait
```

The private directory protects the files on the host; their read permissions
allow both PostgreSQL and the backend's UID 1000 to use their mounted secrets.
`.secrets/` is ignored by Git. Keep a secure copy of these credentials.

| Service      | Local address         | Purpose                                         |
| ------------ | --------------------- | ----------------------------------------------- |
| Frontend     | http://localhost:3000 | Web application; `/api` proxies to the backend. |
| Backend      | http://localhost:3001 | Same-origin JSON API under `/api`.              |
| PostgreSQL   | `127.0.0.1:5432`      | Database and application role: `apodect`.       |
| Kafka        | `127.0.0.1:9092`      | Host broker; containers use `kafka:9092`.       |
| Ethereum RPC | http://localhost:8545 | Local Anvil chain.                              |
| Solana RPC   | http://localhost:8899 | Local validator; WebSocket port 8900.           |
| NEAR RPC     | http://localhost:3030 | Local sandbox chain.                            |

Published development ports bind only to loopback. Kafka does not create topics
automatically; the worker creates `apodect.events`. Its single broker also acts as the KRaft controller; this provides
no high availability.

Use `docker compose logs -f` for startup logs and `docker compose down` to stop.
The localnet container shuts down its nodes and services before exiting.

Localnet stays unready until startup has exported the current deployment and relay
credentials; healthy RPC listeners alone are insufficient.

If startup fails, inspect `docker compose ps -a` and
`docker compose logs --tail=100 localnet backend worker`. A stopped container is
not a ready deployment. Completed deployments survive normal restarts. An
interrupted **first** Ethereum or Solana deployment instead reports an incomplete
bootstrap and refuses to guess which transactions succeeded. For a disposable
local environment, use the explicit chain reset under [Persistent data](#persistent-data)
and start again; do not delete volumes to recover a valuable deployment.

## Using the workspace

Open http://localhost:3000. Signing in creates a profile; linking another wallet
requires a signature from that wallet. Roles belong to each Project: its owner
appoints the manager (the contract’s `authority`), while any agent can create a
local task request. Selecting a network or connecting a wallet does not grant
permissions. For full owner access, link the NEAR owner account and the local
owner wallets used to fund each network. The manager signs through its NEAR
account; an agent signs through the task’s execution network. Projects currently
come from deployment configuration; the web application manages these registered
deployments.

Use **Tasks & queue** to request and approve budgets, then start funding for the
head of the approval queue. **Funding** shows each network’s liquidity and owner
invoices. **Activity** separates submitted operations, completed transfers and
plans needing intervention. A successful wallet transaction does not by itself
mean a cross-chain allocation is ready. Uncertain wallet submissions appear in
**Account & wallets** and must be reconciled before another payment.

Task and activity filters help find open work without changing FIFO priority.
Links connect each budget request, funding plan and task. Before signing, review
the affected record, recipient, amount, costs and deadline; a changed allocation
or permission requires a fresh review. A Solana agent without a USDC receiving
account can create it in the withdrawal transaction; the review shows its address
and maximum SOL rent deposit.

Purpose descriptions and completion reports are stored as immutable text with an
on-chain hash. The author and current verified project owner/manager can read the
text. An external hash may have no description in this deployment.

For local testing, export a disposable wallet and import its JSON through
**Account & wallets → Use a local development wallet**:

```sh
docker compose exec localnet ./scripts/localnet.sh export-wallet ethereum agent
mkdir -p .localnet/wallets
chmod 0700 .localnet/wallets
docker compose cp localnet:/app/.localnet/wallets/agent.ethereum.json .localnet/wallets/
chmod 0600 .localnet/wallets/agent.ethereum.json
```

Replace `ethereum agent` with `near authority` for manager actions, or `near owner`
for project controls. `solana` is also supported. With the native launcher, run
`./scripts/localnet.sh export-wallet <chain> <role>` directly. Exports stay in the
Git-ignored `.localnet/wallets/` directory; the command prints only their paths.
Use separate browser profiles to test separate people. To test one person using
several networks, link their wallets within the same profile.

Imported test keys remain in browser memory and disappear on logout or reload;
the backend never receives them. This importer is available only on loopback in
a simulator deployment. Public networks use installed Ethereum/Solana wallets or
MyNearWallet. Keep the application URL equal to `APP_ORIGIN`; mutations from a
different origin are rejected.

## Development and local workflows

Frontend source edits use Vite hot reload through bind mounts. For Rust source
or Cargo manifest changes, run the Compose watcher in another terminal:

```sh
docker compose watch backend worker
```

It rebuilds and replaces the backend using Docker's cached Cargo build. Re-run
`docker compose up --build --wait` after changing frontend dependencies or build
configuration. Contract changes also need a rebuilt localnet image and fresh
deployments; normal restarts preserve the existing contracts.

Inspect the environment and registered Projects:

```sh
docker compose exec localnet ./scripts/localnet.sh status
printf '%s\n' '{"command":"projects"}' | docker compose exec -T backend apodect
docker compose logs -f worker
```

Compose runs the Rust application worker. The Python relayer and scripted
owner/authority actions belong to the standalone contract-development harness;
Compose does not run a second application relayer. The backend prepares wallet
transactions, while owner, authority and agent wallets sign them themselves.

For host application checks, install Rust 1.99+ with rustfmt, Clippy and a C
compiler and make, plus Node 24 and pnpm 12.18.0:

```sh
npm install --global pnpm@12.18.0
pnpm install --frozen-lockfile
make check-apps
```

`make check-apps` checks the Cargo workspace under `apps/` and checks/builds the
frontend with pnpm. `make check` additionally runs contract and coordination
suites; their host prerequisites are in the
[native development guide](local-development.md).
`make fmt` includes Rust and frontend formatting.

The native `make local-up` launcher remains available for contract development.
Stop Compose before using it. Native data lives in the host's `.localnet/`;
Compose data lives in `localnet_data`. These locations are separate and neither
launcher imports the other's state.

## Live host

Use a server with Docker, the same secret files and a public hostname whose DNS
points to the server. Allow incoming TCP ports 80 and 443; UDP 443 enables HTTP/3.
Create the environment file without replacing an existing one:

```sh
test -e .env.live || cp .env.live.example .env.live
```

Local and live commands operate on the same Compose project and database.
Stop with the configuration that started the stack before switching to the other
configuration.

Set `SITE_ADDRESS`, `APODECT_SOLANA_CLUSTER`, the deployment configuration and
the three gas-payer credential paths in `.env.live`. Copy
[deployment.example.json](deployment.example.json) to `.secrets/deployment.json`
if you are registering an existing mainnet deployment; replace its marked values
and zero creation block following the [provider setup](cross-chain-providers.md#register-an-existing-public-deployment).
The file contains RPC URLs, so keep it private when they include provider tokens.

Build the release images and check the public bindings before starting:

```sh
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml build
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  run --rm --no-deps backend apodect --check-deployment
```

This check returns JSON and exits unsuccessfully when the deployment cannot be
admitted. It uses no database or gas-payer keys and sends no transactions. Fix the
reported configuration or provider capability before starting the stack:

```sh
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  up --build --wait
```

Caddy serves the built frontend over HTTPS and forwards `/api` to the compiled
Rust backend. PostgreSQL, Kafka and the backend have no published ports. The
override removes the localnet service, development mounts and backend watcher;
it does not deploy local test chains or simulated providers on the live host.
The API and worker also verify public deployment admission at startup, and the
live profile rejects configurations selecting local provider simulators. A local
MPC simulator does not establish public Read MPC admission. This is a single-host deployment,
without high availability. Kafka's combined
broker/controller setup is unsuitable for critical deployments requiring redundancy.

For live logs and shutdown, retain the same `--env-file` and both
`-f` options, replacing `up --build --wait` with `logs -f` or `down`.

1Click accepts unauthenticated quotes; an optional partner API key changes its
[fee schedule](https://docs.near-intents.org/integration/distribution-channels/1click-api/authentication).
To use one, save it in `.secrets/liquidity-api-key` with mode `0444` inside the
private `0700` directory, and set `liquidity.api_key_file` in deployment JSON to
`/run/secrets/liquidity_api_key`. Save this additional override as
`.secrets/compose.liquidity.yaml`:

```yaml
services:
  backend:
    secrets:
      - liquidity_api_key
secrets:
  liquidity_api_key:
    file: ./.secrets/liquidity-api-key
```

Append `-f .secrets/compose.liquidity.yaml` to the live Compose commands. Only the
backend mounts this provider credential; the worker uses the persisted quotes.
`APODECT_FEE_WARNING_BPS` configures the soft fee warning (default `500`, or 5%).
Funding proposals show expected fees and the maximum input buffer separately;
warnings do not block an otherwise affordable plan.

## Persistent data

Named volumes retain PostgreSQL data, Kafka data, localnet ledgers and journals,
the generated local gas-payer credentials, and Caddy's certificates/configuration. `docker compose down` keeps these volumes;
`down --volumes` deletes the volumes belonging to that configuration. Keep
valuable data and credentials backed up before any reset.

The backend embeds the initial SQLx migration. During this prerelease phase,
changes to that initial schema require recreating the disposable application
database; production migration history is not being maintained yet.

Database role initialization runs only on an empty volume. Editing a password file
does not rotate an existing database password. The localnet volume likewise
retains deployments across restarts; rebuilding an image does not upgrade them.

For a deliberate local chain reset, stop the application processes too. The
bootstrap rewrites its public configuration for the new genesis; the backend
registers a new deployment identity. Old database projections remain historical.
This deletes test-chain state, deployments, journals and disposable keys:

```sh
docker compose stop frontend backend worker localnet
docker compose rm -f localnet
docker volume rm apodect_localnet_data
docker compose up --build --wait
```

## Database backup and restore

Create a custom-format backup outside the checkout and copy it off the host.
[`pg_dump`](https://www.postgresql.org/docs/current/app-pgdump.html) captures the
application database; retain the secret files separately.

```sh
umask 077
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  exec -T postgres pg_dump -U postgres -d apodect -Fc > ../apodect.dump
```

Restore into an initialized stack where the `apodect` role already exists.
The following commands replace the application database, so take a backup first.
Keep the applications stopped if any restore command fails:

```sh
(
set -e
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  stop frontend backend worker
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  exec -T postgres dropdb -U postgres --if-exists apodect
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  exec -T postgres createdb -U postgres --owner=apodect apodect
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  exec -T postgres pg_restore -U postgres -d apodect --exit-on-error < ../apodect.dump
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  up --wait
)
```

The database is recreated with `apodect` as owner;
[`pg_restore`](https://www.postgresql.org/docs/current/app-pgrestore.html) restores
its archived objects and permissions. For the local configuration, use
`docker compose` without the live environment and override options.
