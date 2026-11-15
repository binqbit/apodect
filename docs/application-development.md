# Run the application

Docker Compose starts the frontend, Rust HTTP API and worker, PostgreSQL, Kafka,
and one `localnet` service containing all three chains and the simulated external
providers. The worker indexes finalized state, relays NEAR-authorized commands and
reconciles receipts. User wallets sign owner, manager and agent actions.

## Local startup

Run commands from the repository root on an x86-64 Linux Docker host with
`io_uring` enabled, Compose 2.24.4 or newer, and OpenSSL. The images include the
application and chain toolchains; you do not need Rust, Node or chain CLIs on the
host. Docker must already be running and accessible to your user. The first build
needs network access and substantial time and disk space. The localnet seccomp
profile allows the three `io_uring` calls required by Agave.

For a new checkout:

```sh
git clone https://github.com/binqbit/apodect.git
cd apodect
```

Local setup needs no `.env.live`, public RPC accounts or real funds. It creates
**Apodect local project**, test wallets with gas, and initial project liquidity.
Public-network configuration is a separate [live host](#live-host) workflow.

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

Wait for the command to finish successfully, then open **http://localhost:3000**.
`docker compose ps` should show all six services running and healthy. With images
already built, local verification took about **2 minutes for empty volumes** and
**30 seconds with saved state**. These are observations on a Linux development
host, not a time limit; the first image build is additional.

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

## Sign in with local wallets

Export the three actors for the walkthrough below. These commands copy generated
test keys from the running container to a private, Git-ignored directory:

```sh
mkdir -p .localnet/wallets
chmod 0700 .localnet/wallets
docker compose exec localnet ./scripts/localnet.sh export-wallet near owner
docker compose exec localnet ./scripts/localnet.sh export-wallet near authority
docker compose exec localnet ./scripts/localnet.sh export-wallet ethereum agent
docker compose cp localnet:/app/.localnet/wallets/. .localnet/wallets/
chmod 0600 .localnet/wallets/*.json
```

| Actor | Wallet network | File in `.localnet/wallets/` | Purpose |
| --- | --- | --- | --- |
| Owner | NEAR | `owner.near.json` | Manage the project and its NEAR funds |
| Manager | NEAR | `authority.near.json` | Approve requests and start funding |
| Agent | Ethereum | `agent.ethereum.json` | Request, withdraw and return task funds |

At http://localhost:3000 choose **Open your workspace → Account & wallets**.
Select the actor's **Wallet network**, expand **Use a local development wallet**
and import its JSON file. Then open **Projects → Apodect local project**.
There is no project-creation or faucet step.

**Switching actors:** choose **Sign out** in Account & wallets before importing
the next actor, or use separate browser profiles. Importing another wallet while
signed in links it to the current person; a second tab does not create a separate
session. Link wallets only when they represent the same person.

The exporter also supports `solana`, and the roles `owner|authority|agent` on each
network. For owner operations on other networks, export and link their corresponding
owner wallets. The web manages projects registered in the deployment configuration.

Exports contain test keys and remain Git-ignored. Imported keys stay in browser
memory until logout or reload; they are not sent to the backend. The importer is
available only on loopback with a simulator deployment. Import the JSON from disk;
the development server blocks private runtime directories. Public deployments use
installed Ethereum/Solana wallets or MyNearWallet.

## Try a complete task

This small walkthrough uses the existing liquidity and three real local contracts.
For each action, choose **Review transaction →**, then **Sign with wallet**, then
**Back to project**. Wait for the finalized result and refreshed project state
before the next step; **Refresh** reloads indexed balances and tasks.

1. **Agent (Ethereum):** in **Tasks & queue → New task**, select Ethereum, request
   **10 USDC**, and describe the work. Sign the request. It appears awaiting approval.
2. **Manager (NEAR):** expand that task and choose **Approve**, keeping the 10 USDC
   amount. After signing, it enters the funding queue. Approval alone does not
   transfer money.
3. **Manager:** choose **Review funding** for the queued request, review the route
   and fee, and sign. Follow **Activity** until funding finishes and the task shows
   10 USDC available. Cross-chain funding can take several minutes while native
   receipts finalize; follow progress rather than submitting funding again. On a
   fresh stack the Ethereum vault starts empty, so this exercises cross-chain
   funding through the local provider simulator.
4. **Agent:** return to the task and **Withdraw** 10 USDC. Then **Return wallet
   funds** for the full 10 USDC to demonstrate unused-budget recovery. Sign each
   operation and wait for it to finalize.
5. **Agent:** choose **Complete task** and sign. The task is completed, its allocation
   is zero, and the returned funds are available to the shared project budget again.
   Transport fees are spent separately, so the total need not equal its starting value.

The owner can inspect **Overview** and **Funding** throughout. For additional
queue, cancellation and pause scenarios, see [How it works](how-it-works.md).

## Stop, resume and diagnose

Inspect startup or a failed service:

```sh
docker compose ps -a
docker compose logs --tail=100 localnet backend worker postgres kafka
```

For chain-node logs, including after `localnet` has exited:

```sh
docker compose run --rm --no-deps localnet ./scripts/localnet.sh logs
```

To follow service output, run `docker compose logs -f`; Ctrl-C stops following
logs, not the services. To stop the stack while keeping data:

```sh
docker compose down
```

Resume with `docker compose up --wait`. Use `up --build --wait` after source changes.
If PostgreSQL restarts, the worker exits after losing its relayer lock; Compose
restarts it to acquire a new lock before resuming synchronization.

| Symptom | What to check |
| --- | --- |
| Missing secret file | Run the database-secret setup above from the repository root. |
| Port already in use | Stop another stack using the listed ports. For the native harness, use `make local-down`. |
| Solana fails with `io_uring` / permission errors | The Docker host must permit `io_uring`; a restricted host or VM can block it despite the included seccomp profile. Use a supported Linux host. |
| Database authentication fails after editing secrets | Saved PostgreSQL roles still use the original passwords. Restore the matching files; for disposable state, use the full local reset below. |
| Incomplete bootstrap or deployed artifact differs from build | Inspect the node logs. Interrupted first deployments or changed contract artifacts require the explicit local reset below. |
| Wallet sign-in fails or the local importer is missing | Use exactly `http://localhost:3000`. After a chain reset, sign out and export/import the new wallets. |

For a **completely fresh local evaluation**, the following deletes local database,
chain and Kafka volumes, including accounts, tasks and test keys:

```sh
docker compose down --volumes
docker compose up --build --wait
```

Keep the database secret files. Re-export wallets and sign out before importing
the new files. For a narrower reset or valuable data, see [Persistent data](#persistent-data).

## Development

Frontend source changes reload through Vite. For backend changes, run:

```sh
docker compose watch backend worker
```

Rebuild with `docker compose up --build --wait` after changing frontend dependencies
or build configuration. Changed contract artifacts require fresh local deployments,
including changes from package versions, dependencies or compiler updates. A normal
restart preserves deployed code. If startup reports that the deployed program or
code differs from the built artifact, use the [chain reset](#persistent-data).

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
account and three gas-payer credential paths. The comments in `.env.live.example`
specify the required key file formats. Prepare the existing public
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
  worker:
    secrets:
      - liquidity_api_key
secrets:
  liquidity_api_key:
    file: ./.secrets/liquidity-api-key
```

Append `-f .secrets/compose.liquidity.yaml` to live Compose commands. The backend
requests and persists quotes; both backend and worker need the key for public
provider admission during startup.
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
existing deployments instead. After resetting chains, sign out and re-export/import
test wallets; previous exports belong to the old deployment.

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
