# Coordination protocol

NEAR authorizes Project budget decisions; each network's contracts hold and release
its tokens. Delivery, evidence and allocation are separate stages. The backend
relays commands and observations; it cannot grant itself spending rights.

## Identity and commands

A fresh Project binds once to its coordinator domain and signing key. The domain
includes the coordinator, Project, network genesis, vault and token, so a command
cannot be replayed against another deployment. A used Project cannot be rebound.
Agents continue to create tasks and withdraw or return tokens on their own network.

The coordinator stores an authorized command before requesting its signature.
Vaults check the signed operation, action, plan, task/request, amount, recipient,
expiry and local state, then retain an immutable result. Reconciliation can read
that result repeatedly without paying twice. Expiry prevents new execution;
task-control nonces prevent an older control from undoing a newer one.

The [shared protocol crate](../contracts/near/protocol/src/lib.rs) defines the wire
format, including unsigned 64-bit token amounts and 32-byte identities. Independent
encoding vectors must agree across contracts, Rust clients and
[Python tooling](../scripts/crosschain/protocol.py) when this format changes.

## Funding lifecycle

1. A finalized local request is observed on NEAR. Authority approval adds it to
   the common FIFO without reserving money.
2. Authority starts Apply for the head request. NEAR reserves the full principal
   and quoted delivery costs before source dispatch.
3. Each route binds its deposit destination, delivery/refund receivers, amounts
   and expiry. Source dispatch and destination receipt are confirmed separately.
4. Partial output stays held. Only the complete approved principal can be
   allocated to the destination task.
5. Confirmed outcomes close remaining holds. Actual refunds and delivery expense
   settle separately from the task's spending.

Delivery fees come from the Project budget. A quoted buffer stays committed until
its actual refund is observed; late refunds retain their original route and plan.
Provider status describes progress, but cannot credit cash. Direct transfers to a
vault do not create attributed funding or route credit.

## Evidence and accounting

Ethereum uses finalized events from the registered vault. Solana uses immutable
receipt accounts owned by the registered program at vault-derived addresses. NEAR
uses authenticated callbacks from its registered vault. Older balance observations
cannot overwrite newer ones, while an older operation receipt can still settle
its own action.

Source cash, transfers in progress and destination receipts represent one movement
of funds. Holds stay unavailable until execution or safe closure is confirmed.
Funding receipts are deduplicated; an owner's invoice response records intent and
does not credit cash. See [provider integration](cross-chain-providers.md) for the
required provenance and finality checks.

## Recovery

A timeout leaves an unknown outcome. Workers retain operation and transaction
identities and reconcile them before proceeding. Observation retries do not
authorize another transfer; reaching the retry limit requires authority review
and does not release the reservation.

Pause, reclaim, continue and cancel take effect after destination confirmation.
They cannot seize money already withdrawn to an agent's independent wallet.
Restoration follows the normal funding queue and does not clear an owner pause.

Abort releases only funds proven to remain controlled. Authority may replace an
invalidation or closure command only after an authenticated Failed or Invalidated
result; unknown operations keep their original identities. Requeuing requires
settled dispatches and actual source refunds, and preserves approval priority.

See [contract rules](contracts.md) for task controls and
[local workflows](cross-chain-development.md) for executable recovery scenarios.
