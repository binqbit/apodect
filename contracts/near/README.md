# NEAR local contracts

This package implements the locally funded lifecycle in [the shared contract plan](../../docs/contracts.md). Each Workspace factory creates separate Project contract accounts. Each Project owns its own balance in one immutable NEP-141 token contract. There is no shared three-chain budget, remote command authorization, 1Click delivery or Read MPC integration in this milestone.

## Build and verify

Requirements: Linux x86-64 or a supported NEAR sandbox platform, Rust 1.99.0, the `wasm32-unknown-unknown` target, and standard native Rust build tools. Initial dependency/sandbox installation needs internet access. No credentials or public-network access are used by tests.

```sh
rustup toolchain install 1.99.0 --profile minimal --component rustfmt,clippy --target wasm32-unknown-unknown
./scripts/check.sh
```

Run these commands inside `contracts/near`. The scripts select the pinned Rust binaries even when a system Cargo appears before rustup on PATH. `check.sh` checks formatting, builds all three Wasm artifacts, runs unit tests and executes the Wasm integration tests in a local NEAR sandbox. Sandbox 2.14.0 is automatically installed by pinned near-workspaces 0.24.0. An optional `NEAR_SANDBOX_BIN_PATH` can select an already installed compatible binary. No `.agent-workspace` file is required. `./scripts/fmt.sh` formats source; `./scripts/build.sh` builds Wasm only.

`project/` contains the Project contract, `workspace/` the factory, `test-ft/` a standards-based local test asset, and `integration/` the actual sandbox scenarios. Generated `artifacts/` files are build outputs. Wasm compilation includes the SDK-required `--cfg near` host path. Build order is important: the factory embeds the just-built Project Wasm, so callers cannot select arbitrary Project code when creating a Project. Rebuild the factory after changing Project code.

## Code organization

The Project has six source modules. Related records and JSON shapes share `state.rs`; their serialization attributes preserve NEAR's distinct [Borsh state and JSON interface formats](https://docs.near.org/smart-contracts/anatomy/serialization). Shared checks and bookkeeping stay together instead of adding a module for each helper.

| Project module | Responsibility |
| --- | --- |
| [lib.rs](project/src/lib.rs) | Contract root, initialization, authority replacement and read-only views |
| [state.rs](project/src/state.rs) | Persisted records, enum ordering, collection keys and simple JSON messages/view types |
| [helpers.rs](project/src/helpers.rs) | Shared role checks, checked accounting, storage charging and event encoding |
| [tasks.rs](project/src/tasks.rs) | Task creation, pause, continuation, controlled recovery and terminal cleanup |
| [budget.rs](project/src/budget.rs) | Budget requests, approval FIFO, full allocation and replenishment invoices |
| [treasury.rs](project/src/treasury.rs) | Actual FT ingress and the complete outflow admission, dispatch and callback settlement lifecycle |

The complete [Workspace factory](workspace/src/lib.rs) fits in one source file: state and views, initialization, child deployment with its callback, failed-creation refunds with their callback, and role/event helpers. The small test FT remains one standards-based fixture.

The six sandbox scenarios share deployment/transaction [support](integration/tests/support/mod.rs) and form three suites: [accounting and input/completion boundaries](integration/tests/accounting.rs), [task lifecycle and asynchronous outflows](integration/tests/lifecycle.rs), and [factory isolation and creation failures](integration/tests/factory.rs). Each contract has one unit-test file containing behavior and fixed compatibility checks. The compatibility fixtures verify root/record Borsh bytes, actual storage keys/values, enum discriminants and JSON shapes against the captured baseline.

## Account creation and native costs

Deploy the factory Wasm to a dedicated account, then call `new(owner_id, token_id)` using that account. Only that account may initialize it. The immutable owner creates Projects with `create_project(authority_id)`, attaching enough NEAR for the factory's new record plus the child account's code/state/native reserve. The resulting account is `p{nonce}.{workspace_account}`; initialization checks that exact identity and its factory predecessor, without storing duplicate Workspace/Project IDs in the Project. Project nonces are monotonic; an admitted failed creation keeps its identity consumed.

Create-account, initial native transfer, code deployment and initialization are one atomic action receipt. A private callback records its known result. Only a known failed receipt creates a refundable native credit; owner claims it through `claim_failed_creation`, with a separate exactly-once refund callback. Successful child funding is never refunded from factory funds. No timeout declares a creation or refund failed.

Register each Project and withdrawal recipient with the exact FT contract before sending tokens. The owner may register the Project directly through the FT's `storage_deposit(account_id, registration_only)` or through Project `register_token`, forwarding the token's required native registration deposit. A registration refund from the FT goes to its immediate caller; when using the Project helper, any excess stays in the Project native reserve. Native storage, one-yocto FT call deposits and transaction gas are separate from token principal.

Data-growing Project methods are payable and charge the exact positive storage increase after flushing cached maps and serializing the root. Excess deposit is refunded to the immediate caller. Fixed-size existing-data updates need no additional storage payment; changing authority to a longer account ID can increase root storage. Withdrawals additionally retain the one yoctoNEAR forwarded to the FT. Released storage is not a per-user refundable storage balance. Return/funding receiver callbacks only update already allocated bounded records; no new user payment is needed to settle an outflow.

## Interface

Amounts use integer base units encoded as JSON decimal strings. Purpose references are nonzero arrays of exactly 32 byte integers; optional report/invoice references use the same array or `null`, with all-zero representing absence. IDs are local JSON `u64` integers; use an integer-safe client rather than JavaScript number arithmetic beyond its safe range.

| Actor | Project methods |
| --- | --- |
| Any agent, acting for itself | `create_task(amount, purpose)` |
| Task agent | `request_budget(task_id, amount, purpose)`, `withdraw(task_id, amount)`, `release(task_id, amount)`, `finalize(task_id, report)` |
| Authority | `approve_request(request_id)`, `reject_request(request_id)`, `apply()`, `mark_out_of_budget(task_id)`, `restore(task_id, amount, purpose)`, `create_invoice(amount, reference)` |
| Owner | `set_authority(authority_id)`, `withdraw_free(amount)`, `cancel(task_id)`, `force_cancel(task_id)`, `reclaim(task_id, amount)`, `continue_task(task_id)`, `respond_invoice(invoice_id, intended, reference)`, `register_token()` |
| Configured FT only | `ft_on_transfer(sender_id, amount, msg)` |
| Contract-private runtime callback | `settle_outflow(operation_id)` |

Use `get_project`, `get_task(task_id)`, `get_request(request_id)`, `get_invoice(invoice_id)` and `get_operation(operation_id)` for views. Project views expose the next IDs and queue bounds so records can be enumerated without an indexer. Bounded NEP-297 `apodect` events describe approvals, allocations, funding, outflows, recovery, lifecycle, invoices and authority changes. Outflow acceptance and settlement share an operation ID. Stop events distinguish `supplement_rejected` and `authority_out_of_budget`.

Owner funding and agent returns are actual token transfers through the token's `ft_transfer_call`, with one of these string messages:

```json
{"kind":"fund"}
{"kind":"fund","invoice_id":0}
{"kind":"return","task_id":0}
```

The FT-provided sender must be the Project owner for funding or that task's agent for a return. Invoice-linked funding requires a closed/responded invoice. Invalid, excessive or late task returns are rejected by the receiver and refunded by the standard FT resolver. Plain unsolicited `ft_transfer` does not credit the Project ledger, and there is no administrative sweep claiming it as free funds.

## Financial and asynchronous boundaries

Approval reserves nothing. Each `apply()` handles one FIFO head, allocating its full amount or waiting; one removed tombstone can be skipped per call. Allocation leaves tokens in the Project account and creates bounded task rights. Returning external tokens and releasing/reclaiming controlled allocation increase Project free funds, without refilling the task's old allowance.

An accepted withdrawal reserves its amount and permanently marks task use before the FT receipt runs. Known later FT failure restores that allocation once while preserving pause/stop state and first use. Concurrent withdrawals have distinct operation records. Private callbacks check runtime results and settled state, never caller-supplied success. Callbacks inspect success without copying unbounded FT return data. Unknown/unexecuted callbacks remain pending; there is no timeout release or administrator override. One accepted withdrawal requires at least 90 Tgas prepaid, including explicit FT and callback gas budgets.

Unit regressions cover mixed task/owner outflows settling out of order, duplicate callbacks, and reclaim while other amounts remain pending. Sandbox action batches verify that pending transfers block finalization and Continue, and that a rejected batch rolls back both local admission and its scheduled token effects.

Pause blocks future withdrawal acceptance and permits returns/releases. It cannot reverse an accepted FT transfer or seize an external wallet. Finalize requires previous funding, zero controlled allocation and no pending task outflow, and removes any remaining live request. Expense is cumulative issuance minus actual recovery, not proof of purchases.

The source exposes no upgrade entrypoint and factory-created Projects receive no access key. Deployment-account keys, chosen token behavior, NEAR execution and operational deployment controls still matter; a source-level statement does not attest any public deployment. No public account or asset has been configured or deployed by these tests.
