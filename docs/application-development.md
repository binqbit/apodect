# Run the application

Docker Compose starts the frontend, Rust HTTP API and worker, PostgreSQL, Kafka,
and one `localnet` service containing all three chains and the simulated external
providers. The worker indexes finalized state, relays NEAR-authorized commands and
reconciles receipts. User wallets sign owner, manager and agent actions.

## Local startup

Run commands from the repository root on an x86-64 Linux Docker host with
`io_uring` enabled, Compose 2.24.4 or newer, and OpenSSL. The images include the
application and chain toolchains. The first build needs network access and
substantial time and disk space. The localnet seccomp profile allows the three
`io_uring` calls required by Agave.

Stop a running native stack with `make local-down` first: both launchers use the
same RPC ports. Create database secrets once; this preserves existing files:

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

The private directory protects credentials on the host; the files must be readable
by PostgreSQL and the application container's UID 1000. `.secrets/` is Git-ignored.

| Service | Local address |
| --- | --- |
| Web application | http://localhost:3000 |
| API | http://localhost:3001/api |
| PostgreSQL | `127.0.0.1:5432` (database/user `apodect`) |
| Kafka | `127.0.0.1:9092` (containers use `kafka:9092`) |
| Ethereum RPC | http://localhost:8545 |
| Solana RPC / WebSocket | http://localhost:8899 / `ws://localhost:8900` |
| NEAR RPC | http://localhost:3030 |

Development ports bind to loopback. The frontend proxies `/api` to the backend.
Keep its URL equal to `APP_ORIGIN` (`http://localhost:3000` by default), because
sign-in messages and API mutations are bound to this origin.

`up --wait` checks initialized local chains and provider configuration, a completed
worker reconciliation, and initial Project projections in the API. The API's
`/api/health/ready` checks PostgreSQL and indexed coordinator/vault views; it does
not continuously certify external RPC or provider availability.

```sh
docker compose ps -a
docker compose logs --tail=100 localnet backend worker
docker compose logs -f
docker compose down
```

Normal shutdown preserves state. An interrupted first Ethereum or Solana deployment
reports an incomplete bootstrap; it does not guess which transactions succeeded.
For disposable state, use the explicit [chain reset](#persistent-data).

## Sign in with local wallets

Open http://localhost:3000. The web manages Projects already registered in the
deployment configuration. See [the user workflow](how-it-works.md) for roles,
requests, approvals and funding.

For testing, export a disposable wallet and import it through **Account & wallets
→ Use a local development wallet**:

```sh
docker compose exec localnet ./scripts/localnet.sh export-wallet ethereum agent
mkdir -p .localnet/wallets
chmod 0700 .localnet/wallets
docker compose cp localnet:/app/.localnet/wallets/agent.ethereum.json .localnet/wallets/
chmod 0600 .localnet/wallets/agent.ethereum.json
```

The command supports `ethereum|solana|near` and `owner|authority|agent`.
Use `near authority` for manager actions or `near owner` for project controls.
For owner operations on other networks, link their corresponding owner wallets.
Use separate browser profiles for separate people; link wallets within one
profile when they belong to the same person.

Exports contain test keys and remain Git-ignored. Imported keys stay in browser
memory until logout or reload; they are not sent to the backend. The importer is
available only on loopback with a simulator deployment. Import the JSON from disk;
the development server blocks private runtime directories. Public deployments use
installed Ethereum/Solana wallets or MyNearWallet.

## Development

Frontend source changes reload through Vite. For backend changes, run:

```sh
docker compose watch backend worker
```

Rebuild with `docker compose up --build --wait` after changing frontend dependencies
or build configuration. Contract changes require a rebuilt localnet image and fresh
deployments; a normal restart does not upgrade contracts.

```sh
docker compose exec localnet ./scripts/localnet.sh status
printf '%s\n' '{"command":"projects"}' | docker compose exec -T backend apodect
```

Compose runs the Rust application worker. The Python relayer and scripted actor
workflows belong to the separate [native contract harness](local-development.md);
its `apply`, `fund` and `smoke` commands are disabled when the Rust worker owns
relaying. Native state lives in the host's `.localnet/`; Compose uses named volumes.
The launchers do not import each other's state.

For host application checks, install Rust 1.99+ with rustfmt and Clippy, a C
compiler and Make, Node 24, and pnpm 12.18.0:

```sh
npm install --global pnpm@12.18.0
pnpm install --frozen-lockfile
make check-apps
```

`make check-apps` checks Rust formatting, Clippy, Rust tests, and frontend
checks/builds. `make check` adds the contract and coordination suites;
`make fmt` formats maintained sources. See [backend services](backend.md) for API,
CLI and integration-test details.

## Live host

The live override runs compiled applications behind Caddy HTTPS and excludes
local chains and simulators. It is a single-host setup with one PostgreSQL server
and one Kafka broker/controller; it does not provide high availability.

Prepare a Docker host, database secrets, and a hostname pointing to the server.
Allow incoming TCP 80/443 (and UDP 443 for HTTP/3). Create the environment file:

```sh
test -e .env.live || cp .env.live.example .env.live
```

Set its hostname, Solana cluster, deployment configuration path, NEAR relayer
account and three gas-payer credential paths. Prepare the existing public
contracts and deployment JSON using [provider setup](cross-chain-providers.md#public-deployment-setup).
Only the worker receives gas-payer keys; user wallets retain owner/manager/agent
keys. Keep credential files readable by UID 1000 inside a private directory.

Local and live commands use the same Compose project and database. Stop with the
configuration that started the stack before switching. Build and check admission:

```sh
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml build
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  run --rm --no-deps backend apodect --check-deployment
```

The read-only check needs no database or gas keys and submits no transactions.
Resolve admission failures before starting:

```sh
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  up --build --wait
```

Caddy serves the web and proxies `/api`. PostgreSQL, Kafka and the backend have
no published ports. API and worker startup also check public admission and reject
simulator configuration. This configuration alone does not prove public signing,
liquidity delivery or refunds; complete the provider guide's funded checks before
using it for user funds.

For logs or shutdown, retain `--env-file .env.live -f compose.yaml -f compose.live.yaml`
and replace `up --build --wait` with `logs -f` or `down`.

### Optional liquidity API key

If the selected provider account uses an API key, save it as
`.secrets/liquidity-api-key` with mode `0444` under the private `0700` directory.
Set `liquidity.api_key_file` in deployment JSON to `/run/secrets/liquidity_api_key`
and save this override as `.secrets/compose.liquidity.yaml`:

```yaml
services:
  backend:
    secrets:
      - liquidity_api_key
secrets:
  liquidity_api_key:
    file: ./.secrets/liquidity-api-key
```

Append `-f .secrets/compose.liquidity.yaml` to live Compose commands. The backend
requests and persists quotes; the worker uses those quotes without this key.
`APODECT_FEE_WARNING_BPS` controls the soft fee warning (default `500`, or 5%);
it does not block an affordable plan or reduce the agent's principal.

## Persistent data

Named volumes retain PostgreSQL, Kafka, local chains/journals, generated gas keys
and Caddy certificates. `docker compose down` preserves them; `down --volumes`
deletes volumes belonging to that configuration. Back up valuable data and keys.
Changing a secret file does not rotate an existing PostgreSQL password: role
initialization runs only on an empty database volume.

The backend currently embeds one initial SQLx migration. During prerelease,
changes to that schema require recreating the disposable application database;
production migration history is not maintained yet.

To deliberately discard **local chain state**, stop the applications too:

```sh
docker compose stop frontend backend worker localnet
docker compose rm -f localnet
docker volume rm apodect_localnet_data
docker compose up --build --wait
```

This removes test-chain deployments, journals and disposable keys. Bootstrap
exports a new genesis/configuration and the backend registers a new deployment
identity; old database projections remain historical. A normal rebuild preserves
existing deployments instead.

## Database backup and restore

Retain database secrets separately and copy backups off the host:

```sh
umask 077
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  exec -T postgres pg_dump -U postgres -d apodect -Fc > ../apodect.dump
```

Restore into an initialized stack where the `apodect` role exists. This replaces
the application database; take a backup first and keep apps stopped on failure:

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

For local backup/restore, use `docker compose` without live environment/override
options. PostgreSQL backup does not include chain ledgers, Kafka or signing keys.
