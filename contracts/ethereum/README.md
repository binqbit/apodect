# Ethereum contracts

This package implements the [local contract milestone](../../docs/contracts.md) in Solidity. The contracts enforce one network's funded Project resources. Cross-chain coordination, delivery, remote commands, and external-wallet control are outside this package.

## Build and verify

Requirements: Bash, Git, and Foundry with Soldeer support. The verified environment uses Foundry 1.7.1 and Solidity 0.8.33. If Foundry is absent and Nix is installed, the scripts obtain Foundry and solc through `nix shell` without changing the user's installed profile. That fallback pins nixpkgs to revision `7dcdd87373d72e7a2173e4db02e20d3d7f495cfd`, which provides the same verified binaries. On other systems install Foundry from its [official instructions](https://getfoundry.sh/introduction/installation/).

From the repository root:

```sh
./contracts/ethereum/scripts/check.sh
./contracts/ethereum/scripts/fmt.sh
```

`check.sh` installs pinned dependencies, checks formatting, compiles production contracts and reports their sizes, then executes all unit, fuzz, and stateful invariant tests. Extra arguments are passed to `forge test`, for example `./contracts/ethereum/scripts/check.sh -vv`. `fmt.sh --check` checks formatting without editing.

The scripts select this package explicitly with `FOUNDRY_ROOT`. An installed solc 0.8.33 is used when available; otherwise Foundry obtains the version pinned in `foundry.toml`. `FOUNDRY_SOLC` can select an existing compatible compiler executable. Dependencies require network access on the first installation. OpenZeppelin 5.6.1 and forge-std 1.17.0 are pinned to exact upstream Git revisions in `soldeer.toml` and `soldeer.lock`; `dependencies/`, `out/`, and `cache/` are regenerable.

## Contracts

`WorkspaceRegistry` binds one exact six-decimal token address. A signer creates a Workspace and becomes its immutable owner. Only that owner can create its Projects; each creation increments that Workspace's Project nonce and deploys a separate `ProjectVault` using CREATE2. The registry records `(workspaceId, projectNonce) → project address` and never holds Project funds.

Each `ProjectVault` has an immutable owner and token, a replaceable authority, independent custody, and its own approval FIFO. The intended deployed asset is USDC. Six decimals are a unit constraint, not proof of token identity; a deployment must bind the intended asset's actual contract address. The supported token model is an exact-amount, non-rebasing ERC-20. Incoming and outgoing transfers check both account balance deltas; fee-on-transfer behavior is rejected atomically. OpenZeppelin SafeERC20 handles ERC-20 return conventions, and all state-changing vault entry points have reentrancy protection.

No upgrade, arbitrary-call, delegated token-allowance, admin balance override, or unsolicited-token rescue method is exposed. Native gas is paid by the transaction sender. An accidental direct ERC-20 transfer does not credit the ledger and has no recovery path in this milestone; use `fund` or `returnFunds` for attributed deposits.

## Source responsibilities

| File | Responsibility |
| --- | --- |
| `src/WorkspaceRegistry.sol` | Workspace ownership, Project nonces, and isolated vault creation |
| `src/ProjectVault.sol` | One Project ledger: storage, caller authorization, task lifecycle, request FIFO, invoices, principal accounting, and reentrancy protection |
| `src/ProjectDefinitions.sol` | Shared enums, records, errors, and events; no storage or executable behavior |
| `src/ExactToken.sol` | Safe ERC-20 movement with exact sender and recipient balance deltas |

Each vault operation keeps its checks and ledger transition together. Private helpers cover rules reused by several operations, including record lookup, request creation/removal, controlled release, and input validation. `ProjectVault` is the single storage owner; its only first-party base contains declarations. `ExactToken` is the external-token boundary and exposes only internal functions compiled into the vault, following Solidity's [internal-library calling model](https://docs.soliditylang.org/en/v0.8.33/contracts.html#libraries). It requires no separate library deployment or linking address.

Solidity consumers import `ProjectDefinitions` for enums, record constructors, errors, and events, for example `ProjectDefinitions.TaskStatus.Active`. Executable selectors remain under `ProjectVault`. The responsibility refactor preserves the wire ABI, selectors, enum order, and storage slots, including packed record members; ABI `internalType` names now identify `ProjectDefinitions` where appropriate. Source namespace changes do not imply a storage migration or upgrade path.

Compilation changes the creation bytecode. CREATE2 predictions must use the exact current build and constructor arguments: the same factory address and salt with earlier vault creation bytecode need not produce the same Project address. No existing deployment is being migrated.

## Interface and lifecycle

Amounts are `uint256` token base units with checked arithmetic. Identifiers begin at one; zero is the absent request/invoice sentinel. Purpose references are nonzero `bytes32`. Optional report and invoice references use zero for absence. References can contain an opaque identifier or a content hash; they do not prove expenditure or conceal predictable content.

| Actor | Calls |
| --- | --- |
| Workspace owner | Registry `createProject` |
| Project owner | `setAuthority`, `fund`, `withdrawFree`, `cancel`, `pause`, `reclaim`, `continueTask`, `respondReplenishment` |
| Current authority | `approveRequest`, `rejectRequest`, `applyNext`, `markOutOfBudget`, `restore`, `requestReplenishment` |
| Any signer | `createTask`, becoming that task's immutable agent |
| Task agent | `requestBudget`, `withdraw`, `returnFunds`, `release`, `finalize` |

Owner has no implicit authority powers; the two roles may be assigned to the same address explicitly. Authority replacement preserves already approved work, pending requests, task allocations, and invoices.

1. Owner approves ERC-20 allowance to the Project vault and calls `fund(amount, 0)`. Actual received tokens become Project free principal.
2. Agent calls `createTask(amount, purposeRef)`. Authority approves the request, appending it to approval-order FIFO without reserving cash.
3. Authority calls `applyNext`. It handles exactly one queue entry. A removed entry advances the cursor once, an underfunded live head waits, and a sufficiently funded head gets its full allocation. Calls return `Empty`, `Skipped`, `Waiting`, or `Funded` and emit the relevant event.
4. Agent withdraws assigned principal to its own address. The first successful withdrawal marks use permanently. Any reverted transfer also reverts first use and accounting.
5. Agent returns external principal with an ERC-20 allowance and `returnFunds`, or releases still-controlled principal with `release`. These distinct operations make Project funds free and never refill the old task allowance.
6. Agent releases all controlled remainder before `finalize`. Finalization requires prior funding, removes any remaining request atomically, and records the report. Expense is derived from the now-frozen issued-minus-recovered counters.

A task has at most one pending/queued request. Rejected initial requests can be replaced. Supplements require first use; rejecting a supplement stops spending. A new request, approval, and full allocation resumes a stopped task while preserving any owner pause. `TaskStopped` emits an explicit reason; out-of-budget also removes the live request and prohibits new funding for that task. Cleanup and completion remain available.

Ordinary owner cancel succeeds only before first use and atomically releases controlled principal. Owner pause blocks spending without releasing funds. Owner can reclaim available controlled principal while paused. Authority `restore` creates a new FIFO request for a previously funded paused active task. Allocation does not clear the pause; owner Continue requires active status and positive available principal. Retained Continue adds no issuance. A stopped state cannot be cleared by Continue.

Replenishment response and funding are separate calls: `respondReplenishment` closes an invoice with full, partial, or zero intended contribution and credits no cash. Owner `fund(amount, invoiceId)` may later attribute real funding to that closed invoice. Failed funding leaves its response closed and funding unchanged.

Views expose tasks, requests, invoices, FIFO entries/cursor, accounted custody, external outstanding principal, and completed task expense. `freeBalance + allocatedBalance` excludes external principal and direct donations. No additional policy cap or cumulative gross issuance ceiling is imposed; integer overflow still reverts atomically.

## Verification boundary

Tests execute compiled contracts and token transfers in Foundry's local EVM. They cover roles, physical Project isolation, FIFO and tombstones, lifecycle transitions, accounting, token failures, fee-token rejection, authenticated reentrancy, and the self-recipient transfer boundary. Fuzz and stateful invariant tests check conservation, terminal rights closure, and FIFO progression across mixed actions. The stateful fixture retains withdrawn tokens in agent test wallets so it can compare external principal with actual balances; real agents may spend those tokens independently.

Unit and fuzz cases are grouped into `ProjectAccounting` (including invoices), `BudgetRequests`, `TaskLifecycle`, `WorkspaceRoles`, and `TokenSafety` suites. `test/support/ProjectVaultTestBase.sol` owns their shared deployment fixture and actor helpers. The invariant assertions remain in `test/ProjectVault.invariant.t.sol`; `test/support/LifecycleHandler.sol` supplies valid mixed actor actions. `test/TestAssets.sol` contains the token doubles used by these suites.

This package has not been publicly deployed or independently audited. Its local checks do not establish cross-chain behavior, production USDC integration, or guarantees over an agent's external purchases. First-party Solidity sources use `UNLICENSED` because the repository has not selected a distribution license; dependency licenses remain their upstream licenses.
