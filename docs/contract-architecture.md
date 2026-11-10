# Contract source architecture

The packages implement the same [local business rules](contracts.md). Files group related implementation at a useful scale: small state types share a file, common operations share helpers, and substantial instruction or transfer flows have their own modules.

A separate file is useful when it makes an operation easier to find and review. A helper is useful when it shares real behavior or names a nontrivial operation. Simple one-use forwarding functions add navigation without improving the implementation and are kept inline.

## Solana

| Location | Responsibility |
| --- | --- |
| `src/lib.rs` | Program identity and instruction dispatch |
| `src/instructions/<instruction>.rs` | That instruction's Accounts context, validation and handler together |
| `src/state.rs` | All five account records, their enums and task state methods |
| `src/helpers.rs` | Reused arithmetic, validation, request/FIFO and controlled-principal operations |
| `src/errors.rs`, `src/events.rs` | Error codes and event definitions |

The instruction is the useful module boundary here: its accounts and execution are read together, following Anchor's [instruction/context model](https://www.anchor-lang.com/docs/basics/program-structure). The few small account records fit comfortably in one file. Shared operations use one helpers file with direct imports.

Token CPIs stay beside the instruction whose actors and accounts they use. Tests group related scenarios, with shared setup, transport and action helpers. The [package README](../contracts/solana/README.md) explains generated client types and account requirements.

## NEAR

The Project has six source files:

| File | Responsibility |
| --- | --- |
| `lib.rs` | Contract root, initialization, authority replacement and read views |
| `state.rs` | Persistent records, enums, storage keys, JSON messages and view types |
| `helpers.rs` | Shared access checks, accounting, storage charging and event encoding |
| `tasks.rs` | Task creation, control, controlled recovery and completion |
| `budget.rs` | Requests, approval FIFO, allocation and replenishment invoices |
| `treasury.rs` | Actual FT ingress and the complete outgoing-transfer lifecycle |

Storage records and JSON types retain their respective serialization attributes; their modest size does not require separate API/state modules. Shared checks and metering use one helper implementation.

The asynchronous transfer boundary remains explicit: acceptance, reservation, dispatch and private callback settlement stay together in `treasury.rs`. The Workspace factory is one cohesive contract in `workspace/src/lib.rs`, containing its records, creation and failed-creation refund flows. The test FT is also one small contract. See the [NEAR README](../contracts/near/README.md) for entrypoints and storage/gas requirements.

## Ethereum

There are four flat source files:

| File | Responsibility |
| --- | --- |
| `ProjectVault.sol` | Project storage, access, task/request/invoice operations and aggregate accounting |
| `ProjectDefinitions.sol` | Shared record, enum, event and error declarations |
| `ExactToken.sol` | ERC-20 transfers with exact sender/recipient balance checks |
| `WorkspaceRegistry.sol` | Workspace identity and isolated Project creation |

A Project vault is one cohesive ledger. Its operations express their transitions directly; private helpers share repeated validation, request management and controlled-principal release. Splitting every action between an external wrapper and a separate domain library adds unnecessary jumps.

The exact-token integration retains a distinct boundary because several funding/withdrawal operations rely on the same transfer checks. Its functions are internal and compile into the vault according to Solidity's [internal-library model](https://docs.soliditylang.org/en/v0.8.33/contracts.html#libraries). There is one storage owner and no separately linked library deployment. Related unit cases share accounting, task lifecycle, request/FIFO, role and token-safety suites. See the [Ethereum README](../contracts/ethereum/README.md).

## Compatibility

Business behavior, storage and transaction interfaces are preserved. The checks cover Solana instruction/account encoding and constraints, NEAR Borsh/JSON/storage keys and callback annotations, and Ethereum ABI/selectors/storage packing.

Existing source clients use instruction-specific Solana context names and flat `FundProject` fields. Solidity consumers use `ProjectDefinitions` for types, events and errors. Those paths survive this consolidation; internal helper and file paths are updated directly without compatibility wrappers. EVM creation bytecode can change, so CREATE2 predictions use the current artifact.

## Verification

Verification after consolidation passed: 38 Ethereum tests, 22 Solana compiled-program tests, and 14 NEAR unit/compatibility tests plus six sandbox scenarios. Formatting, contract builds and interface/state compatibility checks passed. All 80 existing tests remain, and dependencies, lockfiles and check scripts are unchanged.

The subsequent [mechanics review](contract-mechanics-review.md) added ten regression tests and expanded existing scenarios; all 90 tests passed with the same production contract sources and architecture.

The maintained commands are `make check-ethereum`, `make check-solana`, and `make check-near`. Cross-chain synchronization and public deployment remain outside the [local milestone](contracts.md).
