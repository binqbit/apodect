# Local contract implementation

This milestone implements the same locally funded Project lifecycle on Ethereum, Solana, and NEAR. Each deployment authorizes and accounts for its own funds. Cross-chain budget synchronization, NEAR-origin remote commands, 1Click delivery, and Read MPC verification are deferred. These contracts do not yet enforce a single budget shared by three networks.

The user selected this milestone on November 10, 2026, including open task submission and returns that release funds to the Project. The concrete interfaces below are engineering choices under the existing product rules.

## Components and ownership

- A Workspace records its owner and allocates monotonically increasing Project identifiers. A Project belongs to its Workspace and has a separate token vault. Shared code does not mean shared custody.
- The owner creates Projects, chooses the Project authority, funds the Project, withdraws free funds, cancels unused tasks, and controls task pause/reclaim/Continue.
- Only the current authority approves or rejects budget requests, applies the funding queue, marks a task out of budget, restores a paused task's budget, and requests Project replenishment. Owner is not implicitly authority.
- Any agent can create a task using its own authenticated identity and paying the required transaction/storage costs. Approval is the gate to funds; there is no separate agent registry. Only that agent may request its task's budget, withdraw its allocation, return/release funds, and finalize it.
- Owner identity, Project asset, and task agent are immutable in this milestone. Authority replacement preserves existing requests, approvals, allocations, and invoices; only future authority actions use the new identity. Workspace membership and cross-chain address binding are outside this milestone.

Ethereum uses a registry and separate Project contract addresses. Solana uses Workspace/Project/task/request accounts and a separate Project token vault. NEAR uses a Workspace factory and a separate contract account for each Project, so each Project owns its own NEP-141 balance.

Each Project binds one exact token contract or mint. The intended asset is USDC; local tests use standards-compatible test tokens. No token symbol is an identity check. Ethereum supports exact-amount non-rebasing ERC-20 transfers; Solana binds the legacy SPL Token program and a six-decimal mint. Native transaction fees and storage/rent are separate from token principal.

## Resource model and accounting

Project funding is actual token funding by its owner. Available Project cash, rather than an extra nominal spending ceiling, governs allocation. This milestone does not add an independent owner policy cap or a lifetime gross issuance cap. An approval can wait for later funding.

Maintain integer base-unit amounts with checked arithmetic. Solana amounts fit `u64`; Ethereum and NEAR use their native integer representations with explicit bounds. API-facing NEAR amounts are decimal strings.

For each task, distinguish:

- `available`: token principal assigned to the task and still available in the controlled vault;
- `issued`: cumulative full allocations, including later approved restorations;
- `recovered`: cumulative actual external returns plus releases/reclaims of controlled principal;
- pending outflow, where the chain's asynchronous execution requires it;
- external outstanding principal, derivable as `issued - recovered - available - pending_outflow`;
- final expense, equal to unrecovered external principal when the task finalizes.

Only actual attributed transfers credit Project funding or external returns. Direct unsolicited transfers do not create task entitlement or free ledger balance. External returns cannot exceed that task's confirmed external outstanding principal. A release/reclaim reclassifies existing controlled tokens; it is not another token receipt.

Example, in token units:

| Action | Project free | Task available | Issued | Recovered | External outstanding |
| --- | ---: | ---: | ---: | ---: | ---: |
| Owner funds 100 | 100 | 0 | 0 | 0 | 0 |
| Authority approves 60 | 100 | 0 | 0 | 0 | 0 |
| Apply allocates 60 | 40 | 60 | 60 | 0 | 0 |
| Agent withdraws 40 | 40 | 20 | 60 | 0 | 40 |
| Agent returns 15 | 55 | 20 | 60 | 15 | 25 |
| Agent releases remaining 20 | 75 | 0 | 60 | 35 | 25 |
| Agent finalizes | 75 | 0 | 60 | 35 | 25 expense |

Returns become Project free funds and do not refill the task's old allowance. A later approved allocation increases `issued` again. Reclaim/regrant cycles must not inflate final expense.

## Tasks and requests

Use a small lifecycle: active, stopped, out-of-budget, completed, canceled. Keep the owner pause flag and the monotonic first-use marker separate. Record the stop reason in structured events (supplement rejected or authority out-of-budget), without storing another redundant state field. Waiting, queued, funded/ready, and executing are derived from the request, available amount, and first-use state; they are not interchangeable financial facts.

A task is created with a positive initial request and a nonzero 32-byte purpose reference. Optional report/invoice references are also bounded to 32 bytes; an absent/zero optional value means no reference. At most one live request (pending or approved/queued) belongs to a task at a time. Approved request amount and purpose are immutable. Request states distinguish pending, queued, funded, rejected, and removed.

The agent may replace a rejected initial request with a new request. A running agent may submit a supplement. Rejecting a supplement stops the task and blocks further withdrawals; it does not free its existing controlled allocation. Resuming a stopped task requires a new agent request, authority approval, and full allocation. An owner pause remains effective independently throughout that process.

Authority may explicitly mark a nonterminal task out of budget. Its live request is removed, its allocation stays occupied until explicitly returned/reclaimed, and new allocations to that task are disabled. Another task may be created normally. Same-task reopening is not implemented in this milestone.

Nonterminal tasks may return/release funds and finalize even when stopped, out of budget, or paused: these actions retire rights and do not resume spending. Finalization requires a previously funded task, zero controlled remainder, and no unresolved outgoing transfer. It atomically removes any still-unfunded request for that task so completion cannot race with a later allocation. Finalization records an optional bounded report reference and the final expense. Completed/canceled tasks cannot receive a normal task return or change their expense.

## Approval queue and Apply

Each Project has its own approval-order FIFO. Approval appends a request and does not reserve money. Only authority calls Apply. One Apply call handles one queue head; it either allocates that request's full amount or leaves it waiting. Later smaller requests cannot bypass an underfunded head.

A queued request that was removed by cancellation, out-of-budget, or task finalization is a tombstone. Queue advancement may skip only verified removed entries. Implement bounded, constant-work advancement so a caller cannot scan unbounded history or choose a later live request. Approval order is determined by on-chain execution order.

Applying a request atomically moves its full amount from Project free to task available, records issuance, and clears the task's live-request pointer. It does not transfer funds to the agent wallet. Local allocation is immediately usable subject to lifecycle and pause checks.

## Spending, return, completion, and cancellation

The authenticated task agent may withdraw a positive amount up to the task's available allocation to its own address/account. Arbitrary vault calls, third-party payment recipients, delegated token allowances, and direct merchant adapters are not needed for this milestone. The agent independently controls tokens after receipt.

The first accepted local withdrawal permanently marks use as started. On Ethereum/Solana a reverted transaction was not accepted. On NEAR acceptance of an outgoing asynchronous operation marks start, even if its later FT receipt fails; the pending reservation is subsequently restored on a known failure. Sending/signing a transaction off-chain alone does not mark start.

An external return transfers actual tokens from the task agent and immediately releases the accepted amount to Project free funds. A controlled release reduces available task rights and moves that same amount to Project free funds. Both are allowed during a pause and a stopped state. There is no automatic task resumption or old-allowance refill.

Before normal completion the agent explicitly returns external leftovers and releases its controlled remainder, then calls finalize. Finalize does not silently sweep an allocation. Its expense is an accounting convention, not proof of purchases or an empty external wallet.

Ordinary cancel is owner-only and succeeds only before first use. It removes a live request, closes the task, and atomically releases its controlled allocation. Once use started, the separate owner pause path applies.

## Pause, reclaim, restoration, and Continue

Owner Force Cancel pauses one nonterminal task and blocks new withdrawals. It does not release funds. The task agent may still return/release funds, and an already accepted NEAR transfer still has to settle.

Owner may reclaim a specified positive part of a paused task's confirmed available controlled amount into Project free funds. External/pending amounts are excluded. Owner may then withdraw genuinely free Project funds through the ordinary owner withdrawal method.

Authority restoration creates a positive approved restoration request for a previously funded, paused active task and appends it to the same FIFO. It requires no other live request, and it does not resume the task. Apply performs the full allocation as usual. Restoration adds issuance, balanced by any earlier reclaim/recovery.

Owner Continue requires an active paused task, a positive confirmed available allocation, and no unresolved outgoing task transfer. Owner decides whether that available amount is sufficient for its intended next work; no full-original-budget replenishment is required. A retained allocation needs no new approval or issuance. An actual reclaim followed by restoration does. Continue never clears first use or previous expenditure.

Stopped-after-rejection and owner-paused are independent: Continue does not undo a stopped state, and new budget approval does not undo an owner pause.

## Project replenishment invoices

Authority may create a positive replenishment invoice with a bounded optional reference. Owner response closes that invoice regardless of full, partial, or zero intended contribution. No unpaid remainder stays open.

Response and actual funding are separate operations. The response records the intended contribution and optional reference; it never increases free funds. Owner funding may reference a closed invoice, and only the actual accepted tokens are credited. A failed funding operation does not reopen an invoice. Invoices remain Project records after authority replacement; the owner can still respond and new authority can read them. Native storage costs are paid by callers where needed.

## NEAR asynchronous settlement

The configured FT is the only valid `ft_on_transfer` predecessor. Typed messages distinguish owner funding, invoice-linked owner funding, and agent returns; the FT-provided `sender_id` must match the required actor. Invalid or excess amounts are rejected/refunded without creating credit. Data-growing calls prepay their storage; incoming token callbacks update already allocated bounded records.

Outgoing transfers reserve funds and persist an operation before calling `ft_transfer`. Private callbacks authenticate and settle the same operation exactly once. Success records the outgoing effect. Known failure restores the corresponding held amount, preserving a task's pause/stop state. A callback does not send another payment.

While unresolved, an outflow stays unavailable to allocation, return accounting, owner withdrawal, reclaim, and finalize. Lost/unknown callbacks remain pending; timeout does not release funds and there is no administrator method to declare an unproven failure. Force-pause blocks future acceptance but does not reverse an accepted transfer. Storage and callback gas must be provisioned so ordinary settlement does not depend on a new user payment.

## Verification requirements

Every implementation must exercise the common scenarios below through its chain runtime, in addition to useful pure-state tests. Tests use local test assets and identities, with no public deployment or real funds.

| Scenario | Expected result |
| --- | --- |
| Accounting example above | Final expense 25, Project free 75, zero task available |
| Owner versus authority versus agent | No implicit role override or another agent's task access |
| Two Projects | Neither funds nor substituted task/request/vault accounts cross the boundary |
| Approval and FIFO shortage | Approval reserves nothing; smaller/later request cannot bypass |
| Full allocation and repeated Apply | No partial allocation or double issuance |
| Request rejection/removal and queue tombstones | Correct advancement, no later live-head bypass |
| Additional request rejection and restart | Stop blocks spend; new approved/funded request restores active state |
| Ordinary cancel versus first use | Only the pre-use ordering can cancel |
| Pause and retained Continue | Spend blocked; return allowed; Continue adds no issuance |
| Reclaim, restore, Continue | Correct actors/FIFO and balanced recovery/issuance |
| Out-of-budget | Queue exit, no new allocation, cleanup allowed, another task possible |
| Completion | Explicit release required; no repeated finalize or late return/expense rewrite |
| Authority rotation | Old authority loses future powers; existing work and invoices survive |
| Invoice response and actual funding | Partial/zero response closes; response alone creates no cash |
| Wrong token, account substitution, transfer failure | No unauthorized custody or ledger mutation |
| Zero/overflow/excess values and repeat calls | Checked failure without accounting corruption |
| NEAR pending/failure/callback replay | Reserved until verified settlement; failure restores once |

Solana verification must execute the compiled SBF program and SPL Token instructions. NEAR verification must execute compiled Wasm with an FT in sandbox, including failed asynchronous transfers. Ethereum verification must execute contract calls and token transfers in the EVM, with conservation/invariant cases. Formatting, compilation, and chain-runtime test results are reported separately from any future deployed verification.

## Implementation status

The local milestone is implemented and locally verified as of November 10, 2026. The subsequent [mechanics review](contract-mechanics-review.md) maps the agreed rules to code and tests, covers every local state, and identifies the contract changes required for cross-chain integration.

| Package | Verification command from repository root | Verified result |
| --- | --- | --- |
| [Ethereum](../contracts/ethereum/README.md) | `make check-ethereum` | Formatting, compilation, 42 EVM tests: 39 unit/fuzz and 3 stateful invariants |
| [Solana](../contracts/solana/README.md) | `make check-solana` | Formatting, SBF compilation, 24 compiled-program runtime tests |
| [NEAR](../contracts/near/README.md) | `make check-near` | Formatting, three Wasm builds, 18 unit tests and 6 sandbox scenarios |

The maintained suites exercise actual local token transfers and contract execution. Ethereum invariants each ran 128 sequences of 64 calls; fuzz tests used 256 cases. Solana tests execute the compiled SBF program with SPL Token instructions. NEAR tests execute Project/factory Wasm and a standards-based FT, including failed and concurrent asynchronous transfers and failed Project creation refunds. No tests were skipped.

See the [package index](../contracts/README.md) for setup and shared commands, and [source architecture](contract-architecture.md) for responsibility boundaries and refactor compatibility checks. These results cover this local scope; they do not establish public deployment, production asset integration, an independent security audit, or cross-chain synchronization.
