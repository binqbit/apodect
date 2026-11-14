# Coordination protocol

NEAR decides what a Project may do; each chain controls its own tokens. Delivery
and proof of delivery are separate operations. This separation prevents a provider
status, a relayer retry or a delayed observation from creating spendable funds.

## Identity and authorization

A fresh Project binds once to its coordinator domain and signing key. The domain
includes the coordinator, Project identity, chain/genesis, vault and token. This
prevents a valid command for one deployment from being reused in another. A used
Project cannot be silently rebound. In coordinated Projects, management decisions
come from NEAR; agents still create tasks and withdraw/return tokens locally.

The coordinator stores each concrete command before requesting its signature.
Commands bind the operation, action, plan, task/request, amount, recipient and
expiry. Vaults verify the signature and local state, and retain an immutable result.
The same successful operation can be reconciled repeatedly without paying twice;
changing its content is rejected. Expiry prevents new execution but does not erase
an already confirmed result. Task-control nonces prevent delayed controls from
undoing newer ones.

The bounded cross-chain representation uses unsigned 64-bit token amounts and
32-byte identities. Exact command/checkpoint/funding layouts and MPC DTOs live in
[the shared Rust protocol](../contracts/near/protocol/src/lib.rs) and
[Python encoding](../scripts/crosschain/protocol.py). Chain-specific verification
lives with the vault implementations. Keep their independent wire vectors in sync
when changing encoding; matching application round trips alone cannot detect a
shared encoding mistake.

## From approval to allocation

1. A finalized local task request is observed on NEAR. Authority approval adds it
   to the common FIFO without reserving money.
2. Apply considers the head request, available cash and exact-output quotes. It
   reserves principal and delivery costs before issuing any dispatch command.
3. Each route pins its deposit destination, output receiver, refund receiver,
   quoted amounts and expiry. Delivery and refund use separate attributed receivers.
4. Source dispatch and destination receipt are independently confirmed. Partial
   output remains held; the agent receives no partial budget.
5. Only complete principal can be allocated to the destination task. Remaining
   holds close against confirmed outcomes; refunds and transport expense settle
   separately from the task's spending.

Fees belong to the Project. An input buffer remains unsettled until its actual
refund is observed. A late refund retains its original route/plan attribution.
A provider reporting SUCCESS cannot substitute for received tokens. Direct token
transfers to a vault do not create attributed route or funding credit.

## Evidence and accounting

Ethereum evidence binds a finalized receipt to the registered emitter and event.
Solana evidence binds immutable account contents to the registered program and
vault-derived receipt PDA. NEAR reads the registered vault through authenticated
callbacks. Monotonic observations prevent an older aggregate balance from
replacing a newer one; an older operation receipt can still settle its own action.

Source cash, in-transit funds and destination receipts must never be counted as
three balances. Holds remain unavailable until execution or safe closure is
confirmed. Funding receipts are deduplicated by domain and funding ID. An owner's
invoice response records intent and never credits cash.

A successful account read alone is insufficient: the provider interface must prove
the required provenance and finality. [Provider integration](cross-chain-providers.md)
explains the constraints that matter when connecting public services.

## Recovery and task control

A timeout is an unknown outcome, not a failed payment. Relayers keep the original
operation and signed transaction identities and reconcile them before proceeding.
Observation retries never authorize a new funded transfer. Reaching the configured
retry limit requires authority review; it does not free the reservation.

Pause, reclaim, continue and cancel become effective on the destination after
confirmed execution. Cancel cannot reverse an agent withdrawal that already
started. Restoration grants new budget through the normal FIFO and never clears
an owner pause. Funds in the agent's independent wallet remain outside vault control.

Abort releases only funds proven to remain controlled. Explicit authority recovery
may replace an invalidation or closure command with an authenticated Failed or
Invalidated outcome; unknown or pending commands keep their identities. Requeuing
requires settled old dispatches and attributed refunds, and preserves the original
approval position.

Use [local workflows](cross-chain-development.md) to exercise these transitions.
The [coordinator](../contracts/near/coordinator/README.md) explains its deployment
requirements; exact method arguments remain beside the contract code.
