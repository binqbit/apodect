# NEAR budget coordinator

The coordinator manages one common Project budget across Ethereum, Solana and
NEAR. It keeps approval order, plans liquidity movement and creates bounded
commands. The vaults retain token custody and enforce those commands locally;
relayers carry records and evidence without gaining independent spending rights.

This separation lets the coordinator combine cash from several chains while
preserving full principal for an approved task. Its
[contract rules](../../../docs/contracts.md) and
[wire protocol](../../../docs/cross-chain-protocol.md) define the shared behavior.

## Run and check

The [NEAR setup guide](../README.md) covers toolchain and build requirements.
From the repository root, `make check-near` builds and checks the coordinator
alongside custody contracts. `make local-up` deploys and connects the complete
three-chain stack. Use the shared `./scripts/localnet.sh` workflow in the
[development guide](../../../docs/cross-chain-development.md).

Configure provider identities and verification keys before binding vaults.
Command signing uses the coordinator's derived signing key; foreign-state
verification uses the separately pinned Read MPC key. Vault, asset, domain and
key bindings are fixed, so changed bindings require fresh deployment.
The MPC 3.18 interface requires one yoctoNEAR per signing or foreign-verification
request; the caller also funds coordinator storage and callback reserves.
For integration details, use the [contract definitions](src/lib.rs) and
[shared provider/protocol types](../protocol/src/lib.rs).

## Operational constraints

Approval adds intent to FIFO without reserving cash. Plans reserve actual local
funds before dispatch, and allocation waits for the full destination principal.
Each route keeps its own delivery/refund attribution; an HTTP SUCCESS response
or invoice intention creates no funds. Actual refunds can settle after delivery.

Evidence can arrive out of order. A late command acknowledgement may settle its
own operation while a newer balance observation remains authoritative. Keep
native transaction and receipt history so these effects can be reconciled.

Observation retries reuse immutable operations. Authority recovery can replace an
invalidation or closure command only after a proven Failed or Invalidated
outcome; expiry and unknown execution do not justify another transfer.
Storage-growing calls also need native NEAR deposits, which the local tooling supplies.

The local stack simulates MPC and liquidity responses while executing real native
contracts and token transfers. Public provider admission, key derivation and
finality settings require separate integration work; see
[provider boundaries](../../../docs/cross-chain-providers.md).
