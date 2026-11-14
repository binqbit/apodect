# How it works

This guide describes the coordinated project flow shown in the web app.
The manager is called **authority** in the contracts. Separate
[standalone contract mode](contracts.md) is available for local contract development.

## Join a project

Open the workspace, sign in with a wallet and set your name and username in
**Account & wallets**. Additional wallets must be linked with proof of ownership.
Open a project to see your role and available actions.

**Invite people** shares a project link; it grants no permissions. Any agent can
create its own task. To appoint a manager, the owner uses **Replace manager**,
finds the person's @username and selects their verified NEAR wallet. Each project
has one manager; replacing that manager preserves approved tasks and launched plans.

The website manages already deployed, registered projects. Ask the deployment
operator to register a missing project; there is no self-service deployment flow.

## 1. Fund the project

The owner opens **Funding → Add funds**, selects a network and signs the deposit.
Confirmed deposits join the shared budget after synchronization. Each project's
vaults retain their own funds; separate projects cannot spend each other's budgets.

The manager can request replenishment from the owner. The owner's response closes
that request even when the contribution is partial. Closing a request does not
prove a deposit: only confirmed funds can support new allocations.

## 2. Request and approve work

An agent selects **New task**, chooses its execution network, explains the work
and requests a USDC budget. The agent signs on that network. Purpose and report
text is stored by the application with its hash on-chain.

The manager reviews and approves or rejects the request. Approval fixes its amount
and funding priority; it does not move or reserve money. The queue follows
**approval order**, so a later smaller request cannot bypass the waiting head,
even if its funds are already on the right network.

Temporary shortage leaves the request waiting. An explicit **Out of budget**
decision stops the task and removes it from the active queue. The agent can create
a new task through the normal request process.

## 3. Review funding and wait for allocation

The manager selects **Review funding**, checks the allocation, source networks and
delivery costs, then signs Apply. Previewing a plan reserves nothing. Accepted
Apply commits the required funds and creates reservation commands.

The plan uses eligible funds already on the execution network and transfers any
shortfall directly from other project vaults. The project must cover the full
approved amount plus delivery costs. A high-fee warning does not waive this check.
Native gas, rent and storage are paid separately by transaction submitters.

Follow **Activity** for the plan's confirmed progress:

| Stage | Meaning |
| --- | --- |
| Approved | Amount and priority are set; manager funding is still required. |
| Funding | Funds are being reserved, delivered or assigned; they cannot fund another request. |
| Allocated | The full amount is confirmed in the task's local allocation. An eligible, unpaused task can withdraw. |

Source dispatch alone does not prove destination delivery, and delivered funds do
not become a task allocation until assignment is confirmed. Once an earlier plan's
reservations are confirmed, the next eligible request may begin funding without
reusing those commitments.

## 4. Perform the task

The agent withdraws from its confirmed allocation using its own wallet. The vault
limits withdrawals to that allocation. The agent chooses its downstream purchases;
Apodect cannot freeze or automatically reclaim funds in an independent wallet.

The first accepted local withdrawal marks the task as started. Ordinary
cancellation then closes. A failed NEAR token-transfer callback restores the
withdrawal amount but does not undo that first-use marker.

An agent needing more funds submits another budget request for approval. It joins
the ordinary queue. Rejection stops further withdrawals; restarting uses a new
approved request. This differs from an owner-imposed pause.

## 5. Return leftovers and complete

**Return wallet funds** transfers unused USDC back from the agent's wallet.
**Release allocation** frees unused funds still held for the task inside its vault.
Neither refills the agent's old allowance. After confirmation and synchronization,
these funds can support other approved requests; the manager signs their funding.

The agent completes the task after clearing its controlled allocation and pending
outgoing transfers, with an optional report. Expense is issued principal minus
recovered principal, including wallet returns, allocation releases and reclaims.
It records accounting, not independent proof of purchases. Return leftovers before
completion; ordinary completion does not support later expense recalculation.

## Owner controls and recovery

| Action | Effect |
| --- | --- |
| Cancel before use | Permanently cancels an unstarted task and closes its spending rights before its controlled funds become free. |
| Pause withdrawals (Force Cancel) | Blocks local withdrawals once applied. Returns remain allowed; independent wallets are unaffected. |
| Reclaim | Recovers available controlled allocation from a paused task. It cannot seize an agent wallet. |
| Restore budget | The manager authorizes restoration; its confirmed request joins the queue for funding. Restoration does not resume the task. |
| Continue task | The owner resumes an active paused task with a positive allocation and no pending outgoing transfer. A retained allocation suffices; full original funding is unnecessary. |
| Withdraw project funds | Withdraws only free funds, excluding task allocations, reservations and unsettled transfers. |

Remote controls take effect when the local contract applies them. A click or NEAR
submission does not prove that another network has already paused withdrawals.

For a delayed wallet operation, check **Account & wallets → Wallet transactions →
Transaction history** before signing a replacement. For a funding plan, inspect
**Activity**. Unknown results keep funds committed. The default retry limit is
three; reaching it requires attention but does not release funds or stop settlement
cleanup. Retries may recheck an outcome; another funded send requires proof that it
cannot duplicate the financial effect.

See [Architecture](architecture.md) for synchronization and
[contract rules](contracts.md) for exact lifecycle conditions.
