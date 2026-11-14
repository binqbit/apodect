# Local cross-chain workflows

These commands use the **native Python harness**, started with `make local-up`.
For Compose, use the [web workspace or backend CLI](application-development.md):
the Rust worker owns its relaying and disables the harness's `apply`, `fund` and
`smoke` commands. See [native development](local-development.md) for prerequisites,
ports and lifecycle.

Examples use generated owner, authority and agent accounts. Amounts are integer
USDC base units: `1000000` means 1 USDC. Replace request/task/quote IDs with those
returned by the previous command.

## Request and fund a task

```sh
./scripts/localnet.sh request ethereum 5000000
./scripts/localnet.sh approve ethereum REQUEST_ID
./scripts/localnet.sh apply ethereum REQUEST_ID
./scripts/localnet.sh status
```

The agent creates its task on the execution chain. The relayer reports finalized
evidence to NEAR; authority approves it and then starts Apply for the global FIFO
head. Approval alone reserves no cash. Apply returns `Waiting` when principal plus
costs cannot be covered, or reserves funds and grants the full allocation after
confirmed delivery. `--source solana` restricts remote funding to that chain.
Delivery fees come from Project funds separately from the agent's amount.

## Withdraw, return and complete

Once funded:

```sh
./scripts/localnet.sh agent ethereum TASK_ID withdraw 1000000
./scripts/localnet.sh agent ethereum TASK_ID return 1000000
./scripts/localnet.sh agent ethereum TASK_ID release 4000000
./scripts/localnet.sh agent ethereum TASK_ID finalize
```

This example returns the withdrawn 1 USDC and releases the remaining 4 USDC from
the original 5 USDC allocation. Returns/releases free Project funds; they do not
refill the old allowance. The commands also support `solana` and `near`.
The agent's `request` action creates another budget request for an existing task.
See [contract rules](contracts.md) for task controls and completion requirements.

## Add Project funds and answer invoices

```sh
./scripts/localnet.sh fund near 10000000 --id initial-near-funding
./scripts/localnet.sh call open_invoice --args '{"amount":10000000,"purpose":"HEX32"}'
./scripts/localnet.sh call respond_invoice --role owner --args '{"invoice":1,"intended":10000000,"reference":null}'
./scripts/localnet.sh fund solana 10000000 --invoice 1 --id invoice-1-solana
```

Replace `HEX32` with a nonzero 64-character hex reference and use the returned
invoice ID. An owner's response closes the invoice intent, including partial or
zero contributions; only an actual token transfer credits Project cash.

After a lost response, reuse the same `--id`: the adapter recovers its original
receipt without another transfer. Reusing it with changed terms is rejected.
Without `--id`, the CLI creates/stores a new funding intent; a new ID means another
funding instruction.

`call METHOD --role owner|authority|agent --args JSON` invokes coordinator methods
for task controls, manager replacement, invoices and recovery. Exact method
arguments are in the [coordinator](../contracts/near/coordinator/src/lib.rs).
Domain IDs are in `.localnet/coordination/manifest.json`. Cross-chain controls stay
pending until confirmed at their destination. Pause cannot retrieve an agent's
independent-wallet funds.

## Faults and recovery

`make local-smoke` exercises delivery/refund failures, service restart, funding-ID
deduplication and each agent's local lifecycle. It preserves history and spends
test fees; run it with an idle queue and no unfinished plans.

For a controlled fault, configure it before delivery begins:

```sh
./scripts/localnet.sh fault QUOTE_ID partial
./scripts/localnet.sh fault QUOTE_ID partial --release
make local-status
make local-logs
```

Supported modes are `normal`, `partial`, `refund`, `false_success` and `late_refund`.
The maintained smoke scenario pauses relaying while configuring them.

Inspect status before retrying after a timeout. An unknown payment retains its
reservation and original transaction identity. The relayer rechecks evidence every
60 seconds without progress; after the retry limit, authority can review and call
`resume_plan`. Only confirmed failed/invalidated cleanup commands can be replaced
through that explicit action; pending or uncertain commands cannot.

`make local-down` followed by `make local-up` preserves history and resumes
reconciliation. NEAR token funding also requires enough native storage balance;
insufficient storage refunds the incoming FT transfer without crediting Project
cash. Replenish storage before making another funding attempt.
