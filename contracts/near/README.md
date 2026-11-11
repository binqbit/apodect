# NEAR contracts

Each Project holds its own NEP-141 balance in a separate contract account. The
[coordinator](coordinator/README.md) orders the common budget across chains, while
Project contracts enforce custody and agent spending rights. The
[contract rules](../../docs/contracts.md) describe the lifecycle; the
[protocol](../../docs/cross-chain-protocol.md) defines commands and custody evidence.

## Build and check

Use Rustup with Rust 1.99.0, rustfmt, the `wasm32-unknown-unknown` target and
standard native Rust build tools. The pinned near-workspaces dependency installs
sandbox 2.14.0 automatically. Initial setup needs network access.

From the repository root:

```sh
rustup toolchain install 1.99.0 --profile minimal --component rustfmt --target wasm32-unknown-unknown
make check-near
./contracts/near/scripts/fmt.sh
```

The check builds Wasm and exercises real FT transfers in temporary sandboxes;
it needs no persistent node or public credentials. Use the maintained build
script because the factory embeds the Project's compiled Wasm.

## Use the local stack

`make local-up` starts all three chains and creates one NEAR Project with 1,000
local test USDC, bound to the coordinator. Use the common check, smoke, shutdown
and explicit reset commands in the [local guide](../../docs/local-development.md).
The [workflow guide](../../docs/cross-chain-development.md) uses
`./scripts/localnet.sh` for tasks, funding and recovery.

## Integration limits

Register the Project and withdrawal recipients with the exact configured FT
before transferring tokens. Amounts use integer base units and NEP-141 decimal
strings. Native storage, gas and required call deposits are separate from USDC.

Immutable owner-funding receipts consume the Project's native storage reserve.
Replenish it with an ordinary NEAR transfer when needed. If storage cannot be
covered, the receiver fails and the standard FT resolver refunds the tokens;
no funding credit remains. After an ambiguous response, reconcile the original
funding reference before considering another payment.

Outgoing FT transfers reserve principal before execution. A private callback
settles each result once; known failure restores funds, while unknown results
remain reserved. Neither timeout nor owner pause reverses an accepted transfer.
The native client also preserves unresolved signed transactions for reconciliation.

Local archival storage retains transaction/receipt evidence and grows until reset.
Changed contract state or Wasm requires fresh disposable deployments; no automatic
migration or upgrade is provided. For call/DTO details, use the [contract source](project/src/lib.rs)
and [protocol types](protocol/src/lib.rs). Public MPC, asset and delivery integration
remain subject to the [provider boundaries](../../docs/cross-chain-providers.md).
