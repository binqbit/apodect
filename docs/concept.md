# Product overview

Apodect coordinates one project's USDC budget across NEAR, Ethereum and Solana.
A team can fund its overall budget before knowing how much each agent or network
will need. Partners operate their own agents and request funding for their work.

## Why a shared budget

With separate network budgets, one agent can wait for funds while money sits unused
elsewhere. Apodect gives requests a common approval order and moves eligible funds
to the network where the next task needs them. Unused funds returned by one agent
can support another task without a new owner contribution.

The NEAR coordinator tracks approval priority and funding commitments. Local
contracts hold project funds and enforce allocations. Reserved, in-transit and
task-allocated amounts are unavailable for competing requests. Each project has
its own budget; sharing a workspace does not pool different projects' money.

## Roles

| Role | Responsibility |
| --- | --- |
| Owner | Funds the project, appoints its manager and controls cancellation, pause, reclaim and continuation. |
| Manager | The contract's **authority**: reviews requests and explicitly starts funding. |
| Agent | Creates tasks on its execution network, withdraws approved funds, returns leftovers and completes the work. |

Wallet sign-in creates a profile. Contract roles determine permissions; a profile,
username or invitation link does not grant management access. The website manages
projects whose deployments have already been registered by an operator.

## What the budget guarantees

Approval fixes an amount and queue position; funding is a separate manager action.
An agent receives its full approved allocation or waits. Delivery costs are paid
from the project budget separately from that amount, and returned funds do not
refill the agent's old allowance.

The vault limits withdrawals to the task's allocation. Once money reaches an
independent agent wallet, Apodect cannot enforce its purchases or recover it
automatically. Task expense records issued principal minus recovered principal;
it is not proof of the quality or cost of the delivered work.

The current runnable stack uses real local contracts and test-token transfers.
External Chain Signatures, Read MPC and liquidity delivery are simulated locally;
see [provider integration](cross-chain-providers.md) for public deployment requirements.

Continue with [How it works](how-it-works.md) for the user flow or
[Architecture](architecture.md) for component responsibilities.
