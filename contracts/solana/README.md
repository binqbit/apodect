# Apodect Solana contracts

This package implements the locally funded Project lifecycle in [the shared contract plan](../../docs/contracts.md). It uses Anchor accounts and legacy SPL Token custody. Each Project has its own PDA-owned token account, binds one exact six-decimal mint, and accounts only for actual owner funding and attributed agent returns. Every amount is an integer token base-unit `u64`; counters use checked arithmetic. Tokens sent directly to a vault remain outside the free ledger.

Run `./scripts/check.sh` to check formatting, compile SBF, and execute the compiled program and SPL Token instructions in LiteSVM. Run `./scripts/fmt.sh` to format Rust files. No local validator, wallet file, RPC connection, deployment, or test funds are needed. The scripts work without `.agent-workspace` and accept `CARGO_TARGET_DIR` as a build-artifact root; they explicitly separate `sbf`, `host`, and `deploy` outputs. Explicit target paths also support the installed Nix Solana wrapper.

The verified toolchain is Solana CLI 4.0.3 / cargo-build-sbf 4.0.0 / platform-tools v1.53 (SBF Rust 1.89), with host Rust 1.99.1. The program pins Anchor 1.2.1 with Solana v4 dependencies; the runtime suite pins LiteSVM 0.18.2 and requires host Rust at least 1.97.1. Commit and preserve `Cargo.lock`; LiteSVM and Agave dependencies must remain compatible. Anchor's token-account initialization macro requires its Token-2022 module at compile time, while every runtime token-program account is explicitly `Program<Token>` and admits only the legacy SPL program.

The fixed development program ID is `9HLdesoinPvKWr56HRfpbGL9M8yKWzzBFNAr4rbZLkQP`, derived from the SHA-256 bytes of `apodect.solana.local.v1`. It is an identifier for local compilation and testing; no deployment is claimed. A later deployment must select an actual program address and upgrade-authority policy, update the ID consistently, and rerun verification. This program contains no instruction to change its mint/owner/agent, drain occupied funds, bypass roles, or override accounting.

## Code organization

The package follows [Anchor's modular program structure](https://www.anchor-lang.com/docs/quickstart/local):

| Location | Responsibility |
| --- | --- |
| `src/lib.rs` | Program ID and thin public instruction entry points; instruction names and arguments define the existing wire interface |
| `src/instructions/<instruction>.rs` | One purpose-named `Accounts` context beside its handler, with that instruction's signer, account, mint, and PDA checks visible together |
| `src/instructions/mod.rs` | Context exports and the generated account-module reexports required by Anchor's program/CPI macros |
| `src/state.rs` | The five account records, related enums, and account methods; field order remains unchanged |
| `src/errors.rs`, `src/events.rs` | Stable error codes and structured transition events, separate from persisted state |
| `src/helpers.rs` | Shared checked arithmetic, amount validation, request creation/retirement, FIFO append, and controlled-principal release |
| `tests/runtime/` | The compiled-SBF scenarios grouped by accounting/invoices, FIFO, lifecycle, authorization, and asset safety |
| `tests/support/` | SVM signing transport, fixture setup/readers, and a shared actions implementation |

Each instruction now exports its own Rust account-builder type, such as `accounts::ApproveRequest`, `accounts::RejectRequest`, `accounts::Withdraw`, and `accounts::ReturnFunds`; former broad shared context names are removed. `accounts::FundProject` exposes its fields directly instead of nesting the former `treasury` group. These source-client changes preserve flattened on-chain account order, signer/writable flags, optional-account sentinels, instruction bytes, persisted layouts/discriminators, events, and error codes. Event types are imported from `events`, errors from `errors`, and persisted types from `state`. The maintained runtime clients use these instruction-specific builders.

## Accounts and addresses

All addresses use canonical PDA bumps under the program ID. All nonce/index encodings below are little-endian `u64`.

| Account | Seeds | Purpose |
| --- | --- | --- |
| Workspace | `workspace`, owner, caller-supplied nonce | Immutable owner and next Project index; an owner can create multiple Workspaces |
| Project | `project`, Workspace, Project index | Authority, immutable mint, free funds, task/invoice counters and queue endpoints |
| Token vault | `vault`, Project | Separate legacy SPL token account, with Project PDA as transfer authority |
| Task | `task`, Project, task index | Immutable agent, lifecycle, pause/start flags, available/issued/recovered, live request and report |
| BudgetRequest | `request`, Task, request index | Fixed amount/purpose/kind, decision, approval sequence and next queue entry |
| Invoice | `invoice`, Project, invoice index | Requested amount/note, owner response, separate actually received funding |

Accounts are not closed or reinitialized. Task final expense is derived only after completion as `issued - recovered`; no duplicate expense field is persisted. Task external outstanding is `issued - recovered - available`. The independent owner pause flag never resets first use or counters. Purpose references are nonzero 32-byte values; optional invoice/report/response references use 32 zero bytes for absence. References do not enforce purchases or conceal public data.

## Public instructions

The normal Cargo build generates the Rust `instruction::*` and `accounts::*` client builders; no `build.rs` or IDL file is required to use them or run this suite. The `idl-build` feature is available for later IDL generation with a matching Anchor 1.2.1 CLI. This milestone does not ship JSON IDL or TypeScript client artifacts; the installed Anchor 1.0.2 CLI is not used by the check script.

The Rust-generated `instruction::*` and `accounts::*` types implement Anchor instruction/account encoding. `initialize_workspace`, `create_project`, and `set_authority` set up immutable custody and the replaceable authority. `create_task` includes a positive initial request; any signer may submit its own task while paying storage. `submit_request` permits a rejected initial replacement, a supplement after first use, or a restart of a stopped task. There is at most one live request per task.

Authority uses `approve_request` / `reject_request`, `apply_next`, `mark_out_of_budget`, `restore_budget`, and `create_invoice`. Owner uses `fund_project`, `withdraw_free`, `cancel_task`, `pause_task`, `reclaim`, `continue_task`, and `respond_invoice`. The task agent uses `withdraw`, `return_funds`, `release_funds`, and `finalize_task`. Owner is not implicitly authority. Withdrawals transfer only to a token account owned by that exact immutable actor. Returns are real transfers from the task agent; releases/reclaims reclassify the same controlled tokens.

Approval and authority restoration append one queue entry with the exact current `tail` account, including a removed tail. Provide `tail: None` only for an empty queue. `apply_next` accepts only the exact current head and handles exactly one entry: a live head requires its matching task and full funding; a removed head requires `task: None` and advances once. A later head cannot be selected. Queue history is never scanned.

Cancellation, out-of-budget, and finalization require the exact current optional `pending` request account. Omission, substitution, or an extra request fails; successful retirement turns a queued entry into a tombstone. Anchor's generated account builder represents an absent optional account with the program-ID sentinel; callers should use the generated encoding. Finalization requires prior funding and zero controlled remainder, and it removes any live request atomically. Paused/stopped/out-of-budget tasks may return, release, and finalize. Restoring a paused active previously funded task creates a FIFO request and does not continue it. Continue requires positive retained/restored available funds and creates no issuance.

Invoice response and funding are separate instructions. Response closes on partial, full, or zero intended contribution. `fund_project` can optionally attribute an actual owner transfer to a closed invoice of the same Project; the response itself creates no cash. Authority rotation preserves all existing records. Structured `Transition` events identify Project/task/request, action, amount, and explicit rejection/out-of-budget stop reasons; account state remains authoritative.

The runtime tests cover accounting and token conservation; FIFO and queue tombstones; role and Project isolation; correct actor addresses without required signatures; omitted/substituted optional accounts; stops/restarts; cancellation/use ordering; pause/return/reclaim/restoration/Continue; completion and fixed expense; invoice settlement and rotation; real SPL failures; unsupported mints/programs; unsolicited deposits; and zero/excess/overflow values. The tests provide local runtime evidence, without cross-chain synchronization, 1Click, Read MPC, public deployment, or independent security-audit claims.
