# Concept

## One Project budget, independent agents, different networks

Apodect coordinates a Project's budget for independent partner agents working across NEAR, Ethereum, and Solana. A team can set an overall budget even when it does not yet know which agent, task, or network will need each part of it.

The central idea is to make budget allocation a contract-controlled process. NEAR coordinates the Project's common budget and decisions; contracts on participating networks hold Project funds and support local allocations. Partners operate their own agents, rather than handing every task to an Apodect-owned agent.

This document explains the product and its boundaries. The implemented flow runs in the local environment with simulated external providers; [public provider integration](cross-chain-providers.md) requires separate validation.

## The problem

A single Project may involve different kinds of work on different networks. One partner agent might carry out a task on Solana while another works on Ethereum. The team knows how much the Project may use overall, but cannot always divide that amount correctly in advance.

Three problems follow:

- **Fragmented liquidity.** Funds can sit unused on one network while an approved task waits on another.
- **Competing requests.** Several agents can request the same apparent free budget. Approval, funding, and money actually ready to use are not the same event.
- **Incomplete visibility.** A transfer can leave one network before its receipt is confirmed elsewhere. Treating both balances as available, or treating an unknown result as failure, can produce an incorrect budget view.

Apodect addresses these problems with a shared allocation process for each Project, a defined approval order, and a distinction between free funds, committed funds, delivery in progress, and funds assigned to an agent.

## People and responsibilities

The web **workspace** presents registered Projects and the user's verified wallets. Each **Project** has its own allocated budget and separate vault custody. A vault is a contract-controlled place where Project funds are held; this does not imply that there is exactly one vault address across all networks. Wallet sign-in identifies the user; contract roles grant Project permissions.

- The **owner** allocates the Project budget and appoints or replaces its authority. The owner also has the defined cancellation, pause, continuation, and free-budget withdrawal responsibilities.
- The **authority** reviews agents' budget requests and starts the funding plan, called Apply. It can request more Project funding from the owner.
- **Independent partner agents** create tasks, explain their budget needs, use approved allocations, and return remaining funds before completing their tasks.
- **External services and workers** provide integration paths for partner activity. The website provides a human view of the same Project processes.

These roles are distinct. Being the owner does not automatically grant every authority action. A common Workspace does not make every Project's funds one interchangeable pool.

## The approach

An agent requests a budget for a task. Authority approval establishes the amount and its priority, but does not itself reserve cash. The funding queue follows approval order. A later smaller request does not bypass an earlier request that is still waiting for its full amount.

Authority starts Apply to coordinate funding across networks. When sufficient eligible liquidity is available, the amount is committed to the task's funding and delivery. If the full amount is already available locally after higher-priority needs are accounted for, no cross-chain transfer is needed. Otherwise, the plan moves Project funds between networks.

An agent receives its full approved amount or nothing. Funds already committed or in transit cannot be allocated a second time. Delivery fees come from the same Project budget and are recorded separately, rather than deducted from the agent's approved amount.

NEAR's coordination connects these local events into a consistent Project budget view. The contract rules, not one backend's discretionary blockers, define the allocation boundary. This does not remove the need for workers, network access, or verified cross-chain evidence.

## What agents can do with an allocation

Apodect controls the budget issued to an agent, not a fixed list of downstream purchases. Once funds are in the agent's independent wallet, the agent chooses how to use them. A purpose reference explains the request; it is not an enforceable shopping list or proof of execution.

This freedom has a clear limit on Apodect's control: funds outside its controlled vaults cannot be automatically frozen or recovered. Pausing a task does not reverse an external purchase or seize an independent wallet.

Before normal task completion, the agent returns external leftovers and releases its unused vault allocation, then finalizes with an optional report reference. Issued principal minus recovered principal becomes the task's recorded expense. Recovery includes external returns and released or reclaimed allocations. That accounting convention does not prove which purchases occurred or where unreturned money remains.

## What the Project can see

The shared view connects:

- Tasks, requests, approvals, and funding priority.
- Free Project funds, committed allocations, and delivery progress.
- Full amounts ready for agents, actual returns, and completed-task expenses.
- Separately recorded delivery fees and unusually high-fee warnings.
- A known cause of delay, or the last confirmed stage and what still cannot be checked.

This gives the owner and authority a common way to coordinate agents without requiring every agent to know all other tasks, budgets, or networks.

## Initial integration direction and boundaries

The first-version design uses **NEAR Intents 1Click** for cross-chain delivery and selects **Read MPC** for checking relevant states on other networks. Their roles differ: moving funds, confirming facts, and authorizing a local contract action are separate responsibilities.

Local tests cover controlled delivery recipients, refunds, authenticated commands and evidence. Public provider support for these requirements must be verified before deployment. Contract controls do not guarantee agent honesty, merchant quality, a successful purchase, or an automatic refund.

Continue with [Architecture](architecture.md) for component relationships, or [How It Works](how-it-works.md) for the complete Project and task journey.
