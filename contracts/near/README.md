# NEAR contracts

The Workspace factory deploys separate Project accounts, each holding its own
NEP-141 balance. The coordinator manages the shared funding queue and authenticated
commands; route receivers attribute deliveries and refunds. See the
[contract rules](../../docs/contracts.md) and [coordination protocol](../../docs/cross-chain-protocol.md).

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

The check builds Wasm and exercises FT transfers in temporary sandboxes; it needs
no persistent node or public credentials. Use the maintained build script: the
factory embeds Project Wasm, and the Project embeds its route receiver.

## Integration limits

- Register the Project and withdrawal recipients with the configured FT first.
  Amounts use NEP-141 integer decimal strings; native storage and gas are separate.
- Keep native NEAR in the Project for immutable funding receipts, and attach
  storage/callback deposits to data-growing coordinator calls. MPC 3.18 signing
  and foreign-verification requests each require one yoctoNEAR. Insufficient
  funding-receipt storage refunds the incoming FT transfer without ledger credit.
- Outgoing transfers stay reserved until their private callback settles. Confirmed
  failure restores funds; an unknown result stays pending. Reconcile the original
  operation before retrying a payment.
- Preserve native transaction/receipt history. Changed state or Wasm requires
  fresh deployments; no automatic migration or upgrade is provided.

The [Project source](project/src/lib.rs), [coordinator source](coordinator/src/lib.rs)
and [protocol types](protocol/src/lib.rs) define native interfaces. See
[local development](../../docs/local-development.md) for startup and
[provider integration](../../docs/cross-chain-providers.md) for public admission.
