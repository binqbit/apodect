# Local cross-chain workflows

Start the single local stack with `make local-up`. Prerequisites, ports, lifecycle
commands and data locations are in [Local development](local-development.md).
The examples below use its generated owner, authority and agent accounts.
Amounts are integer USDC base units: `1000000` means 1 USDC.

## Request and allocate

```sh
./scripts/localnet.sh request ethereum 5000000
./scripts/localnet.sh approve ethereum REQUEST_ID
./scripts/localnet.sh apply ethereum REQUEST_ID
./scripts/localnet.sh status
```

Use the returned request/task IDs. The agent creates the task on its own chain;
the relayer discovers it and reports finalized evidence to NEAR. Authority approves
it, then Apply handles the global FIFO head. Approval alone reserves no funds.

Apply uses confirmed free balances and exact-output quotes. It returns Waiting
when principal plus costs cannot be covered. Otherwise it reserves funds, arranges
delivery and grants the full approved amount only after delivery is confirmed.
`--source solana` restricts remote funding to that chain. Fees come from the Project,
never from the promised agent amount. `APODECT_FEE_WARNING_BPS` sets the soft warning
threshold (default 500, or 5% of remote principal); the warning does not block an
affordable plan.

## Use and return funds

After the plan is Funded:

```sh
./scripts/localnet.sh agent ethereum TASK_ID withdraw 1000000
./scripts/localnet.sh agent ethereum TASK_ID return 1000000
./scripts/localnet.sh agent ethereum TASK_ID release 4000000
./scripts/localnet.sh agent ethereum TASK_ID finalize
```

Returns and releases free Project funds; they do not refill the old allowance.
The same commands support `solana` and `near`. The agent's `request` action creates
an additional budget request. Tasks created with native contract clients follow
the same discovery flow. See [business rules](contracts.md) for pause, cancellation,
restoration and completion.

## Fund and replenish

```sh
./scripts/localnet.sh fund near 10000000 --id initial-near-funding
./scripts/localnet.sh call open_invoice --args '{"amount":10000000,"purpose":"HEX32"}'
./scripts/localnet.sh call respond_invoice --role owner --args '{"invoice":1,"intended":10000000,"reference":null}'
./scripts/localnet.sh fund solana 10000000 --invoice 1 --id invoice-1-solana
```

Replace `HEX32` with a nonzero 64-character hex reference and use the actual invoice
ID. The owner response closes an invoice's intent, even for a partial or zero
contribution. Only the later token transfer credits cash.

Reuse the same funding `--id` after a lost response: the adapter recovers the
original receipt without another transfer. Changing its chain, amount or invoice
is rejected. Without `--id`, the CLI prints and stores a new intent before sending;
each new ID represents a separate funding instruction.

`call METHOD --role owner|authority --args JSON` exposes coordinator operations
such as task controls, authority replacement, invoice handling and plan recovery.
Use the [coordinator contract](../contracts/near/coordinator/src/lib.rs) for exact
arguments. Domain IDs come from `.localnet/coordination/manifest.json`. A control
request remains pending until the destination confirms it. Pausing cannot recover
funds already held in the agent's independent wallet.

## Exercise failures and recover

`make local-smoke` covers partial output followed by service restart, late refunds,
and a provider reporting success without sending tokens. Each task then withdraws,
returns, releases and finalizes. It also funds one invoice with 1 USDC per chain and
repeats the same funding IDs to check deduplication. Run it while the queue and
existing plans are idle; it preserves history and consumes test transport fees.

The local provider accepts controlled faults for debugging:

```sh
./scripts/localnet.sh fault QUOTE_ID partial
./scripts/localnet.sh fault QUOTE_ID partial --release
make local-status
make local-logs
```

A manual fault must be configured before delivery begins. The maintained smoke
scenario pauses relaying during this setup. Supported modes are `normal`,
`partial`, `refund`, `false_success` and `late_refund`.

After a timeout, inspect status and the existing operation before retrying.
An unknown payment keeps its reservation; it does not authorize a second transfer.
The relayer rechecks evidence every 60 seconds without progress. After the plan's
retry limit is reached, authority reviews it and may call `resume_plan`; original
payment identities remain intact. Confirmed failed/invalidated closure commands
can be replaced, but pending or unknown commands cannot.

Restart with `make local-down` then `make local-up` to preserve ledgers, bindings
and journals. Lost receipts require retained chain history. NEAR funding also
needs native storage balance: insufficient storage refunds the incoming FT transfer
without crediting Project cash. Replenish storage before a new funding attempt.
