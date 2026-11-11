# Backend services

The Rust workspace in `apps/` has three executables: `apodect-backend` hosts
Actix Web, `apodect-worker` reconciles chains and publishes events, and `apodect`
exposes the same services through JSON on stdin. The React application uses the
same-origin `/api` routes. Start the stack with the
[application development guide](application-development.md).

## Ownership and accounting

Owner, authority and agent actions are prepared as unsigned wallet transactions.
The backend holds only separate gas-payer keys. A linked profile never grants an
on-chain role: preparation verifies the current role, and the contract checks it
again when the wallet submits the transaction.

NEAR is the source of cross-chain authorization, FIFO order and reservations.
PostgreSQL stores finalized projections, immutable provider quotes and the
execution journal. It does not maintain a competing editable budget. Source
debit, transport, delivery and allocation remain distinct contract states.

`chains/` handles native RPC and transaction formats. `indexing` builds read
models; `liquidity` prepares exact-output funding proposals; `actions` checks
wallet ownership and constructs user transactions. `operations` orchestrates
reconciliation through `relay`, which persists signed bytes before broadcasting.
This keeps transaction pipelines separate from RPC and encoding details.

Ethereum and NEAR reads are pinned to finalized blocks. Solana RPC does not
offer historical account reads at a chosen slot, so its projection records the
actual range of slots read. Settlement uses immutable receipts with verified
transaction inclusion, account/program or emitter binding and payload identity.
Provider status is diagnostic and never credits funds.

If a native RPC is unavailable, its last confirmed projection stays in place and
the synchronization round remains incomplete. The worker continues observations
and settlement on healthy networks; a failed read never creates available funds.

The worker owns a PostgreSQL advisory lock while using its gas keys. Restarting
it resumes existing transaction identities. An RPC timeout does not mean a
transfer failed, and does not authorize a replacement payment. Definitive native
failures and unresolved execution are visible in the operation journal.
Permissionless coordinator calls may retry a finalized failure or rejected
callback within the coordinator's retry limit; each attempt remains archived.
`retry_operation` lets an operator retry a definitive failure explicitly using
its current `attempt` number. Pending or uncertain transactions cannot be reset.
An expired Solana transaction with no conclusive receipt moves to `attention`
and retains its original bytes while receipt polling continues.

A plan requiring attention stops new financial command submissions and signatures,
including after a worker restart. Receipts still reconcile. If a persisted Ethereum nonce
blocks cleanup, its existing transaction can be rebroadcast once the finalized
chain timestamp exceeds the command deadline; the expired command cannot perform
a new token action. Cleanup may therefore wait for that deadline.

Project changes and their outbox events commit together. Kafka publication is
at least once: consumers must deduplicate the stable event UUID. The outbox row
is deleted only after broker acknowledgement. There are no unused consumers or
placeholder queues.

## Browser interface

The API groups account authentication, registered Project views, immutable text
references and unsigned actions. `/api/projects/{key}/actions/local`,
`actions/coordinator` and `actions/apply` delegate to the existing action services;
the browser does not construct a second set of business rules. Amounts and
contract IDs cross this interface as decimal strings to preserve their precision.

Browser sessions use an HttpOnly, SameSite Strict cookie, with Secure enabled for
HTTPS. Mutations require the configured `APP_ORIGIN`; responses are not cached.
The RPC bridge accepts only the native methods needed by wallet signing and
confirmation, targets configured nodes, and keeps provider credentials private.
Worker retries remain operator actions rather than public API endpoints.

The browser records transaction identity before broadcasting signed bytes. A
wallet that submits internally may lose its response before returning a hash;
that attempt remains unresolved until a matching transaction is found. Recovery
checks the actor, exact action and nonce context before accepting a supplied hash.
A timeout never makes a new payment safe. Public deployment still needs traffic
limits at its trusted ingress and the provider prerequisites documented below.

## Profiles and wallet signing

Profiles use wallet authentication. Ethereum follows SIWE/EIP-191 with ERC-1271
verification for contract wallets; Solana signs the supplied sign-in message;
NEAR uses NEP-413 and a finalized full-access key. NEAR key authority and contract
wallet proofs are rechecked when their sessions are used. Wallet identities
include the network/genesis, so equal addresses on separate networks do not
silently become the same identity.

Challenges expire after five minutes and can be consumed once. Sessions expire
after 24 hours, and only token hashes are stored. Linking requires both the
existing session and a fresh signature from the new wallet. A linked wallet
cannot be reassigned to a different profile.

The CLI reads one JSON object and writes one JSON result. Keep session tokens in
private input files or stdin, rather than command arguments or shared logs:

```sh
printf '%s\n' '{"command":"projects"}' | docker compose exec -T backend apodect
docker compose exec -T backend apodect < /private/request.json
```

Use the deployment `key` from `projects` as `project`. `inspect` reads the
coordinator, `index` refreshes all three local projections, and `operations`
shows execution state without exposing signed transaction bytes. `transport_status`
accepts a Project key and route ID to read provider diagnostics without changing
contract accounting.

Start authentication with `login_challenge` and the `chain` and `address`.
Sign its returned message using that chain's wallet standard; `login` accepts
`challenge`, `signature` and, for NEAR, `public_key`. It returns the session token.
`link_challenge` additionally takes `session`; `link_wallet` consumes that
challenge with the same session. `profile`, `rename_profile` and `logout` use
the session token.

`prepare_local` accepts the session, Project key, actor address, chain and a typed
action: create task, request budget, withdraw, return funds, release, finalize or
fund. NEAR additionally takes a decimal `storage_deposit`; Solana token actions
take the actor's `token_account`. The returned instructions include the exact
spender approval or storage registration where needed. The wallet supplies
fresh gas fees, nonce or recent blockhash and signs the returned transactions in
order. The worker discovers the resulting finalized activity independently.

User action amounts and returned funding costs are decimal strings in token
base units. NEAR intents carry `args_base64`: decode it and pass the resulting
bytes directly to the FunctionCall action. Do not parse and stringify those
arguments in JavaScript; coordinator amounts may exceed its exact integer range.

`prepare_coordinator` prepares the typed owner/authority decisions, including
approval, task controls, invoice handling and plan recovery. `prepare_apply`
combines fresh coordinator balances with validated provider quotes and persists
those quotes before returning the authority's unsigned Apply call. `funding_proposal`
offers the same read/quote workflow for inspection. An unaffordable full request
returns `Waiting`; fees are not subtracted from the agent's requested principal.
The exact JSON variants are defined by the small command enum in
`apps/backend/src/bin/apodect.rs` and action types in `actions`.

## Configuration and checks

`APODECT_CONFIG` points to deployment JSON containing verified network identities,
coordinator/Project bindings, native token addresses and provider asset IDs.
Each Project records `ethereum_start_block`, the exact factory creation block,
so receipt discovery begins at the deployment rather than Ethereum genesis.
Local bootstrap generates it. Public deployments supply it explicitly along
with the provider URL and optional API-key file. Public quotes requiring a
deposit memo are rejected because the current vault dispatch cannot carry one.
Public 1Click routing requires mainnet identities and the native USDC contract
or mint in each chain, matched against the provider catalog and six decimals.
Quote signatures are verified with the production key published by the
[official SDK](https://github.com/defuse-protocol/one-click-sdk-typescript/blob/ae8b24e064785c8cba8081263e03e52fed4de089/src/quote-signature.ts)
before execution terms are accepted. A provider key rotation requires updating
that trusted key. The local simulator uses its explicit separate admission path.

Only the worker mounts `APODECT_*_RELAYER_KEY_FILE` credentials in live Compose.
Localnet exports newly generated gas keys into a separate volume; its owner,
authority and agent keys stay inside localnet. `APP_ORIGIN` binds wallet messages
to the application, and public configuration supplies `APODECT_SOLANA_CLUSTER`.

Run `make check-apps` for formatting, Clippy, Rust tests and the web build.
Database integration tests require a disposable PostgreSQL database via
`APODECT_TEST_DATABASE_URL`; the outbox test also uses `APODECT_TEST_KAFKA_BROKERS`. These tests
create and remove their own schemas, covering replay/link races, atomic
projections and broker-acknowledgement recovery. Run them explicitly with
`cargo test --manifest-path apps/Cargo.toml --workspace -- --ignored`.

The protocol boundaries follow the official
[NEAR transaction RPC](https://docs.near.org/api/rpc/transactions),
[Solana signature status RPC](https://solana.com/docs/rpc/http/getsignaturestatuses),
[SIWE](https://eips.ethereum.org/EIPS/eip-4361),
[NEP-413](https://github.com/near/NEPs/blob/master/neps/nep-0413.md) and
[1Click quote API](https://docs.near-intents.org/api-reference/oneclick/request-a-swap-quote).
Public provider readiness is tracked in [cross-chain providers](cross-chain-providers.md).
