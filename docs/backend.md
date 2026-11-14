# Backend services

The `apps/` Rust workspace has three executables:

| Executable | Responsibility |
| --- | --- |
| `apodect-backend` | Actix Web API used by the frontend |
| `apodect-worker` | Chain indexing, command relaying, receipt reconciliation and event publication |
| `apodect` | Operational JSON interface to the same services |

Use [application development](application-development.md) for startup and
[provider integration](cross-chain-providers.md) for public deployment requirements.

## Authority and persistence

NEAR controls shared approval order, reservations and cross-chain authorization.
PostgreSQL stores finalized projections, immutable quotes and the operation journal;
it is not an editable source of Project budget. The API prepares unsigned user
transactions. Only the worker holds dedicated gas-payer keys; owner, authority and
agent wallets sign their own actions. Profile membership does not grant a role.

Ethereum and NEAR reads use finalized blocks. Solana account views record the
actual slots read because RPC cannot pin historical account reads to a chosen
slot. Settlement checks attributed native evidence; provider status only explains
transport progress. Failed reads retain the last confirmed projection and cannot
create available funds. Healthy networks continue reconciling during another
network's outage.

The worker holds a PostgreSQL advisory lock while using its keys. It persists
signed bytes before broadcast and resumes the same transaction identities after
restart. A timeout or missing receipt never authorizes a replacement payment.
Pending or uncertain operations keep their reservations; expired Solana submissions
without conclusive receipts require attention while receipt polling continues.

Permissionless coordinator calls may retry definitive failures within the retry
limit. Operators can use `retry_operation` with the current `attempt` to retry a
definitive failure; pending/uncertain operations cannot be reset. A plan requiring
attention stops new financial submissions and signatures while reconciliation
continues. Ethereum nonce cleanup may wait until the original command expires.
See [protocol recovery](cross-chain-protocol.md) for the contract rules.

Projection changes and outbox events commit together. The worker publishes to
Kafka topic `apodect.events` and removes an outbox row only after acknowledgement.
Delivery is at least once; consumers must deduplicate its stable event UUID.

## Browser API

The frontend uses same-origin `/api` routes. Sessions use HttpOnly, SameSite Strict
cookies, with Secure enabled on HTTPS. Mutations require `APP_ORIGIN`; responses
are not cached. The RPC bridge forwards only wallet methods allowed by the API
to configured nodes, keeping RPC provider credentials private.

| Routes | Purpose |
| --- | --- |
| `/auth/*`, `/account`, `/account/wallets/*` | Wallet sign-in, profile and verified wallet linking |
| `/profiles/{username}` | Exact username lookup for signed-in users |
| `/projects`, `/projects/{key}` | Registered deployments and indexed Project views |
| `/projects/{key}/references/*` | Immutable purpose/report text bound to on-chain hashes |
| `/projects/{key}/actions/{local,coordinator,apply}` | Prepare unsigned wallet actions |
| `/projects/{key}/routes/{route}/status` | Provider diagnostics |
| `/rpc/{chain}` | Restricted wallet RPC bridge |
| `/health/ready` | Database and initial Project-view readiness |

Readiness returns `204` once every configured Project has an indexed coordinator
and three local vaults, or `503` if indexing/database access is incomplete. It does
not measure continuing RPC/provider health; inspect worker health and synchronization
state for stalled activity. Worker health requires a successful reconciliation and
event-publication cycle within five minutes.

Token amounts cross the API as USDC base-unit decimal strings. Transaction preparation
checks current finalized roles and state; contracts recheck on execution. The
browser records signed transaction identity before broadcast. Wallet submissions
with a lost response remain unresolved until the exact actor/action/nonce context
is reconciled; an arbitrary transaction hash does not establish success.

Purpose/report text is immutable. Its author and current verified Project
owner/manager can read it; a hash created outside this application may have no
stored text. Exact HTTP schemas live in [API handlers](../apps/backend/src/api.rs)
and [authentication handlers](../apps/backend/src/api/auth.rs).

## Profiles and signing

Ethereum authentication uses SIWE/EIP-191, including ERC-1271 contract wallets;
Solana signs the supplied message; NEAR uses NEP-413 with a finalized full-access
key. Contract-wallet and NEAR key authority are rechecked when sessions are used.
Identities include network/genesis, so equal addresses on different networks are
not implicitly the same wallet.

Challenges expire after five minutes and are single-use. Sessions expire after
24 hours; only token hashes are stored. Linking needs an existing session and a
fresh signature from the added wallet. A wallet cannot be reassigned to another
profile. Users may set a display name and unique username. A username makes the
profile and verified addresses discoverable; it grants no Project permissions.

## Operational CLI

The CLI reads one JSON object from stdin and writes one JSON result. Keep session
tokens in private files/stdin rather than process arguments or shared logs:

```sh
printf '%s\n' '{"command":"projects"}' | docker compose exec -T backend apodect
docker compose exec -T backend apodect < /private/request.json
```

Use the returned deployment `key` as `project`.

| Commands | Purpose |
| --- | --- |
| `projects`, `inspect`, `index` | List deployments; refresh coordinator or all chain views |
| `operations`, `transport_status` | Inspect journal state or provider diagnostics |
| `retry_operation` | Explicit retry of a definitively failed operation |
| `login_challenge`, `login`, `logout` | Sign in/out using a wallet proof |
| `link_challenge`, `link_wallet`, `profile`, `update_profile` | Manage a profile and verified wallets |
| `funding_proposal` | Inspect a full funding proposal and quoted costs |
| `prepare_local`, `prepare_coordinator`, `prepare_apply` | Prepare unsigned actor transactions |

The [command enum](../apps/backend/src/bin/apodect.rs) and
[action types](../apps/backend/src/actions.rs) define exact input fields.
`prepare_apply` persists validated quotes before returning the manager's Apply
transaction; an unaffordable request returns `Waiting`. Fees remain separate
from agent principal.

Sign returned transactions in order with the required wallet. It supplies fresh
nonce/blockhash and gas fees. For NEAR, decode `args_base64` and pass its bytes
directly to the FunctionCall action: parsing/restringifying in JavaScript can
corrupt large integer arguments. Solana actions include the receiving token
account and, when needed, its creation and rent deposit.

## Configuration and checks

`APODECT_CONFIG` selects deployment JSON. Project `name` is a display label;
addresses, genesis and binding domains define its identity. Local bootstrap exports
the configuration automatically. See the [deployment example](deployment.example.json)
and [public setup](cross-chain-providers.md#public-deployment-setup)
for manually registered deployments.

The API uses `APP_HOST`/`APP_PORT` (default `0.0.0.0:3001`) and `APP_ORIGIN`.
Use `APP_HOST=127.0.0.1` for a native HTTP launch. Database settings are `PGHOST`,
`PGPORT`, `PGDATABASE`, `PGUSER` and exactly one of `PGPASSWORD` or `PGPASSWORD_FILE`.
The worker additionally uses `KAFKA_BROKERS`, `APODECT_*_RELAYER_KEY_FILE` and
`APODECT_NEAR_RELAYER_ACCOUNT`. Public configuration requires
`APODECT_SOLANA_CLUSTER`. Compose supplies these settings.

Run `make check-apps` for formatting, Clippy, Rust tests and frontend checks/builds.
Database integration tests are opt-in and require a disposable PostgreSQL database
via `APODECT_TEST_DATABASE_URL`; the outbox test also uses
`APODECT_TEST_KAFKA_BROKERS`. They create/remove their own schemas:

```sh
cargo test --manifest-path apps/Cargo.toml --workspace -- --ignored
```

These tests cover replay/link races, atomic projections and broker-acknowledgement
recovery. They do not establish public MPC or liquidity-provider execution.
