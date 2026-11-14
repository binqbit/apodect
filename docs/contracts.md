# Contract rules

Ethereum, Solana and NEAR implement the same Project lifecycle. Each Project
holds one configured token in its own vault. A fresh vault can be bound once to
a NEAR coordinator, which then controls approvals, funding and owner commands
across its registered networks. Agent requests, withdrawals and returns stay
local. Unbound vaults support the standalone lifecycle.

The [coordination protocol](cross-chain-protocol.md) describes command and evidence
handling. This page defines the financial and lifecycle rules those commands enforce.

## Roles and custody

| Role | Responsibilities |
| --- | --- |
| Owner | Creates and funds Projects, appoints authority, withdraws free funds, cancels unused tasks, pauses, reclaims and continues tasks |
| Authority (manager in the UI) | Approves/rejects requests, starts funding, marks tasks out of budget, requests restoration and Project replenishment |
| Task agent | Creates its task, requests budget, withdraws its allocation, returns/releases funds and completes the task |

Any agent may create a task with its own signature and pay its transaction/storage
costs; approval is the gate to funding. Owner and authority are distinct permissions.
Replacing authority preserves existing requests, allocations and invoices.

Workspace factories group Projects without pooling their money. Ethereum uses
separate vault addresses, Solana separate PDA-controlled token accounts, and NEAR
separate Project contract accounts. Owner, asset and task-agent identities are fixed.
Only the configured token contract or mint is accepted; native gas, rent and storage
are separate from USDC principal. See [package constraints](../contracts/README.md).

## Budget and allocation

Actual owner funding creates free Project funds. There is no additional nominal
ceiling or lifetime gross-issuance cap: returned principal can be allocated again.
Approving a request fixes its amount and priority but reserves no money.

Approved requests follow approval-order FIFO. A later smaller request cannot bypass
an underfunded head. Authority starts Apply for the head; the task receives its full
approved amount or nothing. A standalone Apply handles one head, including a removed
entry; coordinated Apply reserves liquidity and confirms any delivery before allocation.
Delivery costs come from the Project budget in addition to the full agent amount.
Allocated funds, reservations and unresolved transfers cannot be allocated twice.

Each task distinguishes funds still available in the vault from principal issued
over its lifetime and principal recovered through returns, releases or reclaim.
Allocation moves Project free funds to the task; withdrawal moves them to its agent.

| Action after funding 100 USDC | Project free | Task available | External principal |
| --- | ---: | ---: | ---: |
| Allocate 60 | 40 | 60 | 0 |
| Agent withdraws 40 | 40 | 20 | 40 |
| Agent returns 15 | 55 | 20 | 25 |
| Agent releases remaining 20 | 75 | 0 | 25 |
| Agent completes | 75 | 0 | 25 recorded expense |

Only attributed, accepted token transfers credit funding or external returns.
Direct donations do not create ledger credit. External returns cannot exceed the
task's confirmed outstanding principal. Returning or releasing funds makes them
free for the Project; it does not refill the task's previous allowance.

## Requests and task completion

A task starts with a positive request and a purpose reference. It may have only
one live request at a time. Standalone rejection accepts only pending requests;
the coordinator can also reject an approved request before funding starts.
Rejecting an initial request allows a replacement;
rejecting a supplement stops further withdrawals without releasing existing funds.
A stopped task resumes through a new request, approval and full allocation.

Authority may mark a nonterminal task out of budget. This removes its live request
and prevents further funding of that task; existing funds remain occupied until
returned or reclaimed. There is no same-task reopening from this state.

Only the task agent can withdraw its available allocation, to its own wallet.
The first accepted withdrawal permanently marks the task as started. On NEAR this
happens when the asynchronous operation is accepted, even if its later FT transfer
fails. Apodect cannot freeze, recover or verify purchases from an independent wallet.

Before completing, the agent returns external leftovers and explicitly releases
its unused vault allocation. Completion requires prior funding, zero controlled
remainder and no pending outflow; it removes any unfunded live request. Expense is
issued principal minus recovered principal, not proof of individual purchases.
Completed and canceled tasks cannot accept normal task returns or revise expense.
Stopped, out-of-budget and owner-paused tasks may still return/release funds and complete.

## Cancellation, pause and restoration

- **Cancel:** owner-only, before first withdrawal acceptance. Closes the task,
  removes its request and releases its controlled allocation.
- **Pause (Force Cancel):** blocks new withdrawals but does not release money or
  reverse an accepted operation. The task remains open for returns and completion.
- **Reclaim:** owner may move a positive part of a paused task's available vault
  allocation back to Project free funds. External and pending funds are excluded.
- **Restore:** authority requests new principal for a previously funded, active,
  paused task with no live request. Standalone restoration is queued immediately;
  the coordinator queues a coordinated restoration after confirming its local
  command succeeded. This authority action supplies approval; Apply is still
  required to fund the request. Funding does not clear the pause.
- **Continue:** owner resumes an active paused task with positive confirmed vault
  funds and no pending outflow. Retained funds need no new approval; the original
  allocation need not be fully restored. First-use history is preserved.

An owner pause and a stopped task are independent: Continue cannot undo a stopped
state, and funding cannot undo an owner pause. Owner withdrawals take only free
Project funds, after any required cancellation or reclaim has settled.
For coordinated Projects, a control takes effect when its command executes on
the task's network; requesting or signing it alone does not change local rights.

## Replenishment invoices

Authority can ask the owner for additional Project funding. The owner's response
closes the invoice even when the intended contribution is partial or zero. That
response is a record, not a deposit; only an actual accepted token transfer credits
the Project. A closed invoice has no outstanding remainder and is not reopened by
a failed payment. Coordinated invoices live on NEAR; standalone vaults keep local ones.

## NEAR settlement

Incoming FT callbacks authenticate both the configured token and the sender.
Invalid/excess transfers are rejected or refunded without credit. Data-growing
calls require native storage funding; coordinated funding receipts use the Project's
native reserve. Insufficient reserve refunds the incoming transfer.

Outgoing FT transfers reserve principal before dispatch. Private callbacks settle
each operation once: success confirms the outflow; known failure restores its
funds without clearing pause or first-use history. Unknown outcomes stay pending.
Timeout does not release money, and no administrator can declare an unproven failure.
Pending funds cannot support allocation, owner withdrawal, reclaim or completion.

Use the [package guides](../contracts/README.md) for builds and checks, and the
[workflow guide](cross-chain-development.md) to exercise these rules locally.
