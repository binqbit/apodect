# How It Works

Apodect coordinates a Project budget across independent agents and networks. This is the intended end-to-end flow, including its waiting, recovery, and control branches. It does not describe a system already deployed or tested.

## 1. Set up the Workspace and Project

The owner organizes work into Projects within a Workspace. Each Project has its own budget and separate vault custody. The owner appoints authority to review agents' budget requests and manage the funding process.

Independent partner agents work under their Project permissions. Different agents can perform different tasks on different networks while drawing on the same Project budget. This does not combine the budgets of separate Projects.

The owner can later replace authority. Already approved tasks and launched deliveries continue unchanged; the new authority receives the role's management rights. This continuity does not grant extra powers to either role.

## 2. Submit a task and request its budget

An agent creates a task under its Project and requests an amount of USDC, with a reference explaining the purpose. The explanation can be represented by an ID or hash; it is not a technical restriction on downstream purchases.

Authority reviews the request and approves or rejects it. Approval fixes the amount and the request's funding priority. It does **not** reserve actual funds, promise immediate delivery, or start the agent's work.

If an unavailable amount is reported, the agent can revise its plan and submit a new request for approval. A displayed availability amount is a current observation, not a reservation that the agent can claim later without the normal checks.

## 3. Follow approval order

Approved requests follow a strict first-in, first-out order based on **approval**, not request arrival. If the first waiting request lacks its full amount, a later smaller request cannot take funds ahead of it. A task does not gain priority merely because money is already present on its network.

An approved task may wait for free funds or Project replenishment. The agent receives the full approved amount or nothing; waiting does not turn approval into a smaller grant.

Authority can explicitly mark a task **out of budget**. That stops the task, removes it from the active queue, and ends further allocation to it in that state. It no longer blocks the remaining queue. The agent can create another task through the ordinary request process. Temporary shortage alone does not automatically trigger this decision.

## 4. Start Apply and prepare funding

After approving requests, authority starts **Apply**, the coordinated funding step. The system considers approval priority, Project liquidity on each network, existing commitments, and the costs of delivery.

Before committing or sending funds, the Project must be able to cover the full approved amount **plus the applicable delivery costs**. Otherwise funding waits; the agent amount is not reduced.

Apply alone does not reserve money. When sufficient eligible liquidity exists for a task, the amount is committed for dispatch and enters delivery. The commitment must remain occupied so the same funds cannot be sent or allocated again.

There are two funding paths:

- **Local allocation.** If the task's full amount is already in the Project vault on the execution network, and remains available after higher-priority needs are accounted for, the contract can assign it locally without a cross-chain transfer.
- **Cross-chain funding.** The plan gathers or moves Project funds from other networks to make the full allocation available where the task will use it.

Local presence never overrides queue priority. Funds on Solana, for example, may be needed for an earlier approved task elsewhere before a later Solana task can receive them.

Independent transfers can progress in parallel when they do not conflict with priority or funding commitments. One task's confirmed full allocation can become ready without waiting for unrelated transfers. The exact sequencing and concurrency controls remain engineering work, rather than an exception to the no-bypass rule.

Apply coordinates funding; it does not itself perform the partner agent's downstream business operation.

## 5. Deliver, synchronize, and make the allocation ready

The first-version delivery choice is NEAR Intents 1Click. The intended confirmation path uses Read MPC to check relevant other-network states. Actual supported routes, controlled recipients, refunds, and the required evidence still need validation.

The system distinguishes these milestones:

1. Funds are committed to the approved funding operation.
2. A source action is submitted and its result is checked.
3. Funds may be in transit, without being free at either end.
4. The destination receipt is confirmed.
5. The full amount is assigned locally to the task and made ready for the agent.

A successful source transaction is not proof of destination delivery. A destination balance is not by itself proof that the task owns an allocation. The same funds are counted once as their location and status change.

NEAR reconciles confirmed local outcomes with the Project view. Until the required facts are confirmed, the system must not display money as freely reusable or an allocation as ready simply because a service expects success.

## 6. Pay delivery fees without cutting the agent's amount

Fees are paid from the **Project budget** and recorded separately. If the Project has 100 USDC, the agent is approved for 60 USDC, and the accounted delivery cost is 1 USDC, the intended result is 60 for the agent and 39 remaining in the Project.

Those numbers are illustrative, not a route quote. The actual fee currencies, gas funding, and conversions must be handled by the implementation.

A configured high-fee threshold warns authority; crossing that warning threshold alone does not block the operation. The Project must still have sufficient funds for both the approved amount and applicable costs. A fee shortfall is a Project funding shortage, not permission to reduce the agent's amount or spend unavailable money.

## 7. Perform the task

The agent may use controlled funds when they are assigned and locked for that agent or when the task is executing, subject to the task's control state. The vault limits issuance to the approved budget.

After withdrawal into an independent wallet, the agent chooses its own operations. Apodect does not enforce a purchase list through the purpose reference. It also cannot automatically freeze, reclaim, or prove the use of funds that have left controlled custody.

A local first accepted operation is the selected boundary for the start of use. This matters for ordinary cancellation; the exact handling of failed operations and concurrent requests must be implemented and tested.

## 8. Request additional funding when needed

An executing agent can request more budget for its task. The extra request needs approval and then enters the ordinary approval-order queue, without privileged priority because the task is already active.

If the additional-budget request is declined, the task becomes stopped and the reason is recorded separately. A later restart uses a new budget request and approval. This stopped-task path is distinct from an owner-imposed Force Cancel pause; neither path automatically releases uncertain funds or starts work again.

## 9. Replenish the Project

Authority can request additional Project funding from the owner through a replenishment invoice on NEAR. The request describes the amount and may include an explanatory reference.

The owner can respond with full or partial Project funding and may attach a reference explaining the response. After the owner's response, the invoice closes regardless of the amount contributed; any difference is not an open remainder on that invoice.

Closing the invoice does not prove that funds have arrived. Only confirmed eligible funding can support subsequent allocations. Partial funding of the **Project** does not authorize partial issuance to an **agent** or change its approved amount.

## 10. Return remaining funds, then complete

For normal completion, the agent first returns remaining funds into the vault. It then finalizes the task and may attach a report reference.

The system records the actual returned amount. Issued funds minus returned funds are recorded as the completed task's expense. The report does not replace evidence of receipt, and the expense label does not prove a particular purchase or the current location of unreturned funds.

The selected normal flow does not include completing first and later returning forgotten leftovers to revise the expense. Return comes before finalization.

Completion, cash return, and closure of previous spending rights are separate facts. Returned funds become eligible for reuse only when the relevant rights are confirmed closed and the funds are actually free. Eligible returned Project funds then serve other approved requests through the ordinary queue. Receiving a report or changing a task label does not create free cash.

## 11. Cancel before use, or pause with Force Cancel

### Ordinary cancel

Before the agent starts spending, the owner can cancel the task through ordinary cancel. Once use has started, ordinary cancel is no longer available. The separate Force Cancel path remains available.

An ordinary cancellation must account for local state and close relevant rights before its controlled remainder is treated as free. A cancellation request is not proof that a remote action was canceled; already accepted external operations are not automatically reversed.

### Withdraw free Project funds

The owner may withdraw only genuinely free Project funds. Funds already allocated to an agent who has not started must first pass through ordinary cancellation and confirmed closure of the relevant rights before becoming free. Ordinary withdrawal does not take an executing agent's allocated budget. Reclaim after Force Cancel is a separate permission limited to available controlled funds.

### Force Cancel and Continue

**Force Cancel is a pause, not permanent cancellation.** The owner pauses the task, may reclaim its available controlled budget, and decides when that task continues.

While the pause is effective, the agent may return funds into the vault but may not withdraw or spend those funds from it. Authority may restore budget to a task, but restoring funds does not itself resume it. Owner Continue is a separate decision, and it does not grant owner every authority power.

Some tasks can remain paused without a restored budget. Continue requires a confirmed available returned reserve; while the relevant money is in transit or unconfirmed, the task remains paused. The exact required amount and treatment of retained versus restored allocations remain implementation details. Changing control alone must not reset earlier use or count the same budget twice.

Apodect cannot forcibly recover money from an independent agent wallet. Those returns depend on the agent. A remote pause takes effect only when the relevant local controls have applied it, not instantly across every network when the owner clicks. Pending external actions require their own outcome checks.

## 12. Resolve delays and unknown outcomes safely

When funding stops progressing, the user should see either a known reason or the last known stage and what could not be verified. No response from a network or delivery provider is not proof that the transfer failed.

Unknown funds remain committed while the system investigates. The default policy allows up to three retries before notifying authority, and the retry setting is configurable. A retry may be another check; it is not automatic permission for another funded transfer.

A transfer can be sent again only when doing so is known to be safe without a duplicate financial effect. Exhausting retries, sending an alert, or seeing a timeout does not release the funds. Delivery, a refund, a return, or rights closure must be reconciled from the relevant confirmed outcome.

## Example: adapt a request and gather funds across networks

This is an illustrative journey, not an executed test or a fee quote.

1. An agent asks for more budget than is currently available. The system reports the available amount rather than issuing an oversized allocation.
2. The agent reprioritizes its intended work, explains the revised plan, and submits a new request for approval.
3. Assume the Project has 40 USDC free on NEAR and 30 USDC free in controlled storage on another network. The revised task needs 60 USDC on Solana.
4. After approval and when its priority permits funding, the demonstration collects 20 from the other network, combines it with the 40 on NEAR, and sends the assembled 60 toward Solana.
5. The display distinguishes collection, delivery, confirmed arrival, and full local readiness. Committed funds are not counted as free during the process.
6. The agent performs its revised work, returns remaining funds, and then finalizes with an optional report. The Project records returns, task expense, and delivery costs separately.

The 40 + 20 = 60 example describes principal movement. Delivery costs are paid separately from the Project's eligible funds, so actual affordability must include them. The other source network, precise route, purchases, and actual costs are not fixed by this example. A request for 60 should not be shown as inherently unaffordable when 70 is safely available and the relevant costs are covered; the initial over-budget request is a separate branch.

Read [Concept](concept.md) for the purpose and role model, and [Architecture](architecture.md) for the component and synchronization paths.
