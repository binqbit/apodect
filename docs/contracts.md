# Contract rules

The contracts implement the same local Project lifecycle on Ethereum, Solana, and NEAR. A fresh Project can also be bound once to a NEAR coordinator and its signing key. That mode places approval, the common FIFO, reservations, invoices and owner controls under coordinator commands, while agent actions and custody remain local. The [coordination protocol](cross-chain-protocol.md) specifies this boundary; the [cross-chain environment](cross-chain-development.md) runs it with real local tokens and explicit delivery/MPC simulations. The standalone lifecycle below remains supported.

## Components and ownership

- A Workspace records its owner and allocates monotonically increasing Project identifiers. A Project belongs to its Workspace and has a separate token vault. Shared code does not mean shared custody.
- The owner creates Projects, chooses the Project authority, funds the Project, withdraws free funds, cancels unused tasks, and controls task pause/reclaim/Continue.
- Only the current authority approves or rejects budget requests, applies the funding queue, marks a task out of budget, restores a paused task's budget, and requests Project replenishment. Owner is not implicitly authority.
- Any agent can create a task using its own authenticated identity and paying the required transaction/storage costs. Approval is the gate to funds; there is no separate agent registry. Only that agent may request its task's budget, withdraw its allocation, return/release funds, and finalize it.
- Owner identity, Project asset, and task agent are immutable in this milestone. Authority replacement preserves existing requests, approvals, allocations, and invoices; only future authority actions use the new identity. Workspace membership is outside this implementation. Coordinated domains explicitly bind each chain vault and asset.

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

An unbound Project has its own approval-order FIFO; coordinated Projects share the NEAR queue. Approval appends a request and does not reserve money. Only authority calls Apply. One Apply call handles one queue head; it either allocates that request's full amount or leaves it waiting. Later smaller requests cannot bypass an underfunded head.

A queued request that was removed by cancellation, out-of-budget, or task finalization is a tombstone. Queue advancement may skip only verified removed entries. Implement bounded, constant-work advancement so a caller cannot scan unbounded history or choose a later live request. Approval order is determined by on-chain execution order.

Local allocation atomically moves the full amount from Project free to task available, records issuance, and clears the task's live-request pointer. It does not transfer funds to the agent wallet. Local allocation is immediately usable subject to lifecycle and pause checks. Coordinated Apply first completes reservations and any required delivery before issuing that allocation.

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

The configured FT is the only valid `ft_on_transfer` predecessor. Typed messages distinguish owner funding, invoice-linked owner funding, and agent returns; the FT-provided `sender_id` must match the required actor. Invalid or excess amounts are rejected/refunded without creating credit. Data-growing calls prepay their storage. Standalone incoming token callbacks update already allocated bounded records. Coordinated owner funding also creates immutable receipts and optional intent references from the Project's native storage reserve; insufficient reserve refunds the FT transfer without crediting funds. The owner can replenish that reserve and retry.

Outgoing transfers reserve funds and persist an operation before calling `ft_transfer`. Private callbacks authenticate and settle the same operation exactly once. Success records the outgoing effect. Known failure restores the corresponding held amount, preserving a task's pause/stop state. A callback does not send another payment.

While unresolved, an outflow stays unavailable to allocation, return accounting, owner withdrawal, reclaim, and finalize. Lost/unknown callbacks remain pending; timeout does not release funds and there is no administrator method to declare an unproven failure. Force-pause blocks future acceptance but does not reverse an accepted transfer. Storage and callback gas must be provisioned so ordinary settlement does not depend on a new user payment.

## Development

The [package guides](../contracts/README.md) explain toolchains and checks.
[Local development](local-development.md) starts the complete coordinated stack;
[workflows](cross-chain-development.md) show task, funding and recovery operations.
Contract suites exercise these rules in the EVM, compiled Solana SBF and NEAR Wasm.
