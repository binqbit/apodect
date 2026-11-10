# Local contract mechanics review

Reviewed November 10, 2026 against the agreed product mechanics and the subsequently selected [independent local milestone](contracts.md). The review covers all production contracts, local state transitions, role checks, custody, queue operations and failure handling on Ethereum, Solana and NEAR.

The local foundation implements the selected local lifecycle. No concrete production logic defect or missing required local transition was found in this review. Ten added regression tests and expanded existing scenarios cover previously untested combinations; all 90 tests passed. Production contract sources, interfaces and stored layouts are unchanged.

Cross-chain integration will require contract changes. The existing interfaces do not yet authenticate NEAR-origin commands, reserve cross-chain dispatches or recognize routed receipts. Local conformance does not establish those capabilities.

## Requirement coverage

The implementation locations and maintained tests below are evidence for the local behavior. The complete target process remains described in [How It Works](how-it-works.md) and [Architecture](architecture.md).

| Requirement | Local implementation and observed boundary | Verification locations |
| --- | --- | --- |
| M01. Separate Project custody | Ethereum factory deploys separate vaults; Solana uses a separate Project token account; NEAR factory creates a separate Project account. Cross-Project funds and task identities cannot be substituted. | [EVM roles][evm-roles], [Solana authorization][sol-auth], [NEAR factory][near-factory] |
| M02. Owner, authority and agent separation | Owner funds/manages authority and pause; authority approves/applies/restores; immutable task agent requests/withdraws/returns/finalizes. Owner has no implicit approval override. | [EVM roles][evm-roles], [Solana authorization][sol-auth], [NEAR boundaries][near-accounting] |
| M03. Open task submission | Authenticated agent creates its own task with a positive amount and nonzero purpose reference; creation also creates the first pending request. Transaction/storage costs are paid by callers where required. | [EVM requests][evm-requests], [Solana authorization][sol-auth], [NEAR accounting][near-accounting] |
| M04. One live request | Initial rejection permits a replacement; approved request amount/purpose do not mutate; an executing task can request an incremental supplement. A second live request fails. | [EVM requests][evm-requests], [Solana lifecycle][sol-life], [NEAR lifecycle][near-life] |
| M05. Approval-order FIFO | Approval does not reserve cash. Only authority applies the exact local head. A smaller later request cannot bypass an underfunded head; supplements and restorations have ordinary priority. | [EVM requests][evm-requests], [Solana queue][sol-queue], [NEAR accounting][near-accounting] |
| M06. Full allocation, once | Allocation moves the full approved amount from Project free funds to task availability; it transfers no tokens to the agent wallet. Repeated Apply cannot issue the same request again. | [EVM requests][evm-requests], [Solana queue][sol-queue], [NEAR accounting][near-accounting] |
| M07. Controlled agent access | Only the task agent can withdraw, within availability, while active and unpaused. Positive partial withdrawals of an already fully allocated budget are supported. External spending is the agent's responsibility. | [EVM accounting][evm-accounting], [Solana accounting][sol-accounting], [NEAR outflows][near-life] |
| M08. Actual returns release Project funds | External returns require an actual authenticated token transfer and cannot exceed confirmed outstanding principal. Controlled release moves reserved principal to Project free funds. Neither refills the old allowance. | [EVM accounting][evm-accounting], [Solana accounting][sol-accounting], [NEAR accounting][near-accounting] |
| M09. Ordinary cancellation and first use | Owner may cancel before the first accepted withdrawal, atomically retiring a live request and releasing controlled funds. First use remains permanent. Reverted EVM/Solana transactions do not mark use; accepted NEAR outflows do even if the FT transfer later fails. | [EVM lifecycle][evm-life], [Solana lifecycle][sol-life], [NEAR outflows][near-life] |
| M10. Force pause and reclaim | Owner pause blocks further withdrawals without releasing allocations. Return/release remain allowed. Reclaim is limited to confirmed available controlled principal, excluding external and pending funds. | [EVM lifecycle][evm-life], [Solana lifecycle][sol-life], [NEAR outflows][near-life] |
| M11. Restore and Continue | Authority restoration enters FIFO and preserves owner pause. Owner Continue requires active/paused state and positive available reserve; NEAR additionally requires no pending task outflow. Retained reserve needs no new issuance. Earlier spend is never reset. | [EVM lifecycle][evm-life], [Solana queue][sol-queue] and [lifecycle][sol-life], [NEAR lifecycle][near-life] |
| M12. Rejected supplement and resumption | Rejection stops withdrawals without freeing existing allocation. Resumption requires a new request, approval and full funding; it does not lift an independent owner pause. | [EVM requests][evm-requests], [Solana lifecycle][sol-life], [NEAR lifecycle][near-life] |
| M13. Out-of-budget and queue exit | Authority stops the task and retires its live request; the removed queue entry can be skipped. Existing allocation stays occupied until recovery. Another task can be created. Same-task reopening is outside the selected local implementation. | [EVM lifecycle][evm-life], [Solana lifecycle][sol-life], [NEAR boundaries][near-accounting] |
| M14. Completion and fixed expense | Agent returns the external remainder and releases controlled remainder before finalize. Finalize requires prior allocation, zero controlled/pending principal and retires a live request. Expense is issued minus actual recovery. Late normal returns and repeated completion fail. | [EVM lifecycle][evm-life] and [accounting][evm-accounting], [Solana lifecycle][sol-life], [NEAR boundaries][near-accounting] |
| M15. Authority replacement | Only owner replaces authority. New authority controls future role actions; existing approvals, task allocations and invoices survive. Old authority cannot submit new authority actions. | [EVM roles][evm-roles], [Solana accounting][sol-accounting], [NEAR accounting][near-accounting] |
| M16. Replenishment invoices | Owner response closes an invoice even for partial/zero intended contribution. Response creates no cash. Only actual funding credits principal; failed funding does not reopen the invoice. | [EVM accounting][evm-accounting], [Solana accounting][sol-accounting], [NEAR accounting][near-accounting] |
| M17. Failure, arithmetic and asynchronous reservation | Wrong asset/account, excessive/zero amounts and overflow are rejected. EVM/Solana failed transactions roll back. NEAR persists pending outflows, authenticates private callbacks, settles once, and restores known failures without clearing pause/first use. Unknown results remain occupied. | [EVM token safety][evm-safety] and [invariants][evm-invariants], [Solana asset safety][sol-safety], [NEAR unit tests][near-unit] and [sandbox outflows][near-life] |

Production entrypoints are in [ProjectVault.sol][evm-vault], [Solana instruction modules][sol-instructions], and NEAR [tasks][near-tasks], [budget][near-budget] and [treasury][near-treasury]. The factories and state organization are mapped in [Contract source architecture](contract-architecture.md).

## State coverage

### Tasks

| State | How it is reached | Permitted next behavior |
| --- | --- | --- |
| Active, not funded | Task creation or a rejected initial request | Initial request replacement after rejection; approval/allocation; owner cancel; owner pause; authority out-of-budget |
| Active, funded | Initial/restoration allocation; fully funded resumption | Agent withdraw when unpaused; return/release; supplement after first use; owner control; completion after rights cleanup |
| Stopped | Rejected supplement | No withdrawal; new request/approval/full allocation can restore Active; return/release/finalize and owner pause/reclaim remain available |
| OutOfBudget | Explicit authority action | No new allocation or withdrawal; return/release/finalize, owner pause/reclaim, or ordinary cancel if never used; a new task can be created |
| Completed | Agent finalize after cleanup | No new financial rights, expense rewrite or normal late return |
| Canceled | Owner ordinary cancellation before first use | No new financial rights; controlled allocation already returned to Project free funds |

`paused` and `started` are independent flags, not competing lifecycle states. A stopped task can also be owner-paused. Budget restoration never clears pause; Continue never clears first use or reverses Stopped/OutOfBudget. Waiting, queued, ready and executing are derived from request state, available principal and the first-use flag. `Active` alone does not imply spendability. EVM's `None` enum variant identifies an absent record, not an additional task phase.

Returning the external remainder before finalize is the agreed agent workflow. Contracts enforce closure of controlled/pending rights and recognize actual returns; they cannot determine which independently withdrawn tokens the agent spent. The final expense therefore remains the selected accounting convention, not proof of purchases or an empty external wallet.

### Requests and outgoing operations

- A request starts Pending, becomes Queued on approval and Funded on full allocation. Rejection applies to Pending; cancellation, out-of-budget and completion retire a live Pending/Queued request as Removed. Removed queue entries advance individually without scanning or bypassing a live head. Authority restoration creates an already approved Queued request.
- A NEAR outgoing operation moves from Pending to Succeeded or Failed only through an authenticated runtime callback. Pending task principal is excluded from available and confirmed external principal. Pending owner principal is excluded from free funds. Repeated settlement has no second financial effect; an unresolved operation stays Pending.
- An invoice goes from open to closed on owner response. Subsequent actual funding is separately accounted and does not reopen it.

## Required changes for cross-chain integration

These are missing integration capabilities, not implemented placeholders or a finalized transport design. They must be delivered with the next stage before a shared budget can be claimed.

| Required capability | Current boundary | Acceptance condition for integration |
| --- | --- | --- |
| Authenticated remote decisions | Owner/authority operations authenticate a local caller or signer. There is no NEAR command envelope, verifier or consumed-command state. | Bind commands to source, destination network/contract, Project, task/request, amount and action. Reject forged, duplicated and superseded commands; preserve role separation and defined authority-rotation behavior. |
| Common identity and queue ownership | Every deployment has its own Project/task IDs and approval-order queue. Local cash alone is sufficient for current local Apply. | Bind satellite Projects to the coordinating Project and enforce shared approved priority. A local task must not consume cash committed to earlier work elsewhere. Specify the fate of existing local approval/invoice paths. |
| Source dispatch and commitments | Free withdrawal pays the owner; agent withdrawal pays the task agent. Neither is a cross-chain delivery primitive. | Reserve principal and delivery costs against a uniquely identified dispatch before external effects. Unknown outcomes must remain occupied; source debit and destination receipt must not be double-counted. |
| Destination delivery recognition | Ethereum/Solana owner funding and NEAR FT funding accept the configured owner. Unsolicited vault balances are not credited. | Attribute an authenticated routed receipt exactly once to the correct Project and operation; activate the full approved allocation only when eligible. If the chosen transport supports partial arrivals, track them without granting partial task rights; otherwise reject or avoid unsupported partial delivery. A raw balance increase is insufficient. |
| Evidence, finality and lifecycle acknowledgement | Local state/views/events expose current effects. They do not prove cross-chain finality or that an intended pause/cancel has taken effect elsewhere. | Validate the selected Read MPC evidence path and its required predicates. Bind evidence to operation/chain/contract/asset and freshness/finality. Reuse funds only after confirmed rights closure; handle delayed or reordered acknowledgements. |
| Delivery fees, retry and diagnostics | Native transaction/storage costs are caller-funded locally. No Project delivery-fee ledger, route planner or backend retry policy exists. | Account Project delivery costs separately while preserving full agent principal and solvency. Apply the soft fee warning, safe retry limit/configuration and authority escalation without treating timeout as failure proof. |

The exact outbound authorization mechanism and queue/admission integration remain engineering choices for that stage. The agreed Read MPC and 1Click directions do not themselves provide an implemented command authorizer. Relaying a message through a worker does not grant that worker owner or authority powers.

### Existing boundaries to preserve

Returned funds can be approved again, so cumulative `issued` can exceed a Project's original deposit without violating conservation. Local cash is the current limit; no independent nominal policy cap is implemented. Token amount conversions must respect the destination's integer range, including Solana `u64`, when building a multi-chain plan.

The Solana `approval_count` and request `approval_sequence` are observational numbering; FIFO uses `head`, `tail` and `next`. Their removal can be considered with the upcoming state/interface design. This review retains the current layout. Invoice counters supply local identifiers while the selected independent local invoice flow exists; they are not a requirement to duplicate invoices after NEAR coordination is introduced.

Lost NEAR callbacks remain conservatively Pending. No timeout or administrative override declares them failed. A future recovery path requires evidence that preserves the no-duplicate-effect rule; a retry counter alone cannot release their money.

Ethereum vaults expose no upgrade entrypoint; factory-created NEAR Projects have no access key or upgrade method. The integration changes therefore belong in the artifacts before their deployment, or require a separately designed migration for already deployed accounts. None of these local checks deployed a public Project.

## Executed verification

| Package | Executed maintained check | Result |
| --- | --- | --- |
| Ethereum | `make check-ethereum` | Format/build PASS; 42 tests: 39 unit/fuzz and three invariants. Fuzz ran 256 inputs; each invariant ran 8,192 calls with zero unexpected reverts. |
| Solana | `make check-solana` | Format/SBF build PASS; 24 tests executing the compiled program and SPL Token instructions in LiteSVM. |
| NEAR | `contracts/near/scripts/check.sh` (the `make check-near` entrypoint) | Format/three Wasm builds PASS; 18 unit/compatibility tests and six actual sandbox scenarios. |

All 80 earlier tests are retained; ten named regressions were added in existing suites. None were skipped. Ethereum additions cover every restricted vault operation's actor, supplement priority, cross-agent reuse of returned funds, and failed owner withdrawal rollback. Solana additions cover repeated return/regrant and both cancel/use orders within one transaction; the paused OutOfBudget cleanup scenario was expanded. NEAR additions cover mixed/out-of-order callbacks and pending-state guards; sandbox assertions also prove action-batch rollback prevents scheduled FT effects.

The existing architecture and dependencies are preserved. These are local runtime, accounting and conformance results. Live asset configuration, authenticated cross-chain execution, finality and provider behavior require the next stage's own verification.

[evm-vault]: ../contracts/ethereum/src/ProjectVault.sol
[evm-roles]: ../contracts/ethereum/test/WorkspaceRoles.t.sol
[evm-requests]: ../contracts/ethereum/test/BudgetRequests.t.sol
[evm-accounting]: ../contracts/ethereum/test/ProjectAccounting.t.sol
[evm-life]: ../contracts/ethereum/test/TaskLifecycle.t.sol
[evm-safety]: ../contracts/ethereum/test/TokenSafety.t.sol
[evm-invariants]: ../contracts/ethereum/test/ProjectVault.invariant.t.sol
[sol-instructions]: ../contracts/solana/src/instructions
[sol-auth]: ../contracts/solana/tests/runtime/authorization.rs
[sol-queue]: ../contracts/solana/tests/runtime/queue.rs
[sol-accounting]: ../contracts/solana/tests/runtime/accounting.rs
[sol-life]: ../contracts/solana/tests/runtime/lifecycle.rs
[sol-safety]: ../contracts/solana/tests/runtime/asset_safety.rs
[near-tasks]: ../contracts/near/project/src/tasks.rs
[near-budget]: ../contracts/near/project/src/budget.rs
[near-treasury]: ../contracts/near/project/src/treasury.rs
[near-unit]: ../contracts/near/project/src/tests.rs
[near-accounting]: ../contracts/near/integration/tests/accounting.rs
[near-life]: ../contracts/near/integration/tests/lifecycle.rs
[near-factory]: ../contracts/near/integration/tests/factory.rs
