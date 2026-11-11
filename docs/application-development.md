# Application development

One Compose command starts the React frontend, Rust/Actix Web backend,
PostgreSQL, Kafka and the local blockchain environment. The `localnet` service
runs Ethereum, Solana and NEAR, deploys their contracts, and starts the relayer
and simulated MPC/liquidity providers.

The frontend displays Hello World. The backend checks PostgreSQL through SQLx
at startup and has no business endpoints yet. Kafka is available, but application
producers, consumers, topic definitions and contract integration are not implemented.
Local blockchain transactions are real; external provider behavior is simulated.

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

| Service      | Local address         | Purpose                                             |
| ------------ | --------------------- | --------------------------------------------------- |
| Frontend     | http://localhost:3000 | Hello World; `/api` proxies to the backend.         |
| Backend      | http://localhost:3001 | Returns `404` until routes are implemented.         |
| PostgreSQL   | `127.0.0.1:5432`      | Database and application role: `apodect`.           |
| Kafka        | `127.0.0.1:9092`      | Broker for host tools; containers use `kafka:9092`. |
| Ethereum RPC | http://localhost:8545 | Local Anvil chain.                                  |
| Solana RPC   | http://localhost:8899 | Local validator; WebSocket port 8900.               |
| NEAR RPC     | http://localhost:3030 | Local sandbox chain.                                |

Published development ports bind only to loopback. Kafka does not create topics
automatically. Its single broker also acts as the KRaft controller; this provides
no high availability.

Use `docker compose logs -f` for startup logs and `docker compose down` to stop.
The localnet container shuts down its nodes and services before exiting.

## Development and local workflows

Frontend source edits use Vite hot reload through bind mounts. For Rust source
or Cargo manifest changes, run the Compose watcher in another terminal:

```sh
docker compose watch backend
```

It rebuilds and replaces the backend using Docker's cached Cargo build. Re-run
`docker compose up --build --wait` after changing frontend dependencies or build
configuration. Contract changes also need a rebuilt localnet image and fresh
deployments; normal restarts preserve the existing contracts.

Inspect or exercise the container's blockchain environment with its existing CLI:

```sh
docker compose exec localnet ./scripts/localnet.sh status
docker compose exec localnet ./scripts/localnet.sh smoke
docker compose exec localnet ./scripts/localnet.sh request ethereum 5000000
```

The smoke flow creates task history and spends test transport fees; run it while
the queue is idle. The [workflow guide](cross-chain-development.md) covers approval,
funding and recovery. Prefix its `./scripts/localnet.sh` commands with
`docker compose exec localnet` to use this environment.

For host application checks, install Rust 1.99+ with rustfmt, Clippy and a C
compiler, plus Node 24 and pnpm 12.18.0:

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

Set `SITE_ADDRESS` in `.env.live` to your hostname, then start:

```sh
docker compose --env-file .env.live -f compose.yaml -f compose.live.yaml \
  up --build --wait
```

Caddy serves the built frontend over HTTPS and forwards `/api` to the compiled
Rust backend. PostgreSQL, Kafka and the backend have no published ports. The
override removes the localnet service, development mounts and backend watcher;
it does not deploy local test chains or simulated providers on the live host.
This is a single-host deployment, without high availability. Kafka's combined
broker/controller setup is unsuitable for critical deployments requiring redundancy.

For live logs and shutdown, retain the same `--env-file` and both
`-f` options, replacing `up --build --wait` with `logs -f` or `down`.

## Persistent data

Named volumes retain PostgreSQL data, Kafka data, localnet ledgers and journals,
and Caddy's certificates/configuration. `docker compose down` keeps these volumes;
`down --volumes` deletes the volumes belonging to that configuration. Keep
valuable data and credentials backed up before any reset.

Database initialization runs only on an empty volume. Editing a password file
does not rotate an existing database password. The localnet volume likewise
retains deployments across restarts; rebuilding an image does not upgrade them.

To redeploy changed contracts, delete only the localnet volume and rebuild its
container. This deletes test-chain state, deployments, journals and disposable
keys while preserving PostgreSQL and Kafka data:

```sh
docker compose stop localnet
docker compose rm -f localnet
docker volume rm apodect_localnet_data
docker compose up --build --wait localnet
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
  stop frontend backend
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
