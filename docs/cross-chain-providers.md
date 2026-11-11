# Provider integration

The local stack executes real contracts and test-token transfers. Two external
capabilities are simulated: NEAR MPC signing/foreign verification, and a liquidity
provider following the 1Click exact-output route model. These simulators exercise
the application boundary; they do not establish a public MPC quorum or liquidity.

## Chain Signatures and Read MPC

The implemented interface follows [NEAR MPC 3.18.0](https://github.com/near/mpc/releases/tag/3.18.0).
The coordinator requests signatures for commands it has already authorized.
The relayer cannot ask it to sign arbitrary actions. Signing and foreign-reading
use different key domains; their request/response types are defined in the
[shared protocol crate](../contracts/near/protocol/src/lib.rs).

Public signing keys derive from the actual predecessor account, derivation path
and domain. Configure the key derived for the coordinator account, not a worker
account. The foreign-verification key is the separate domain-3 root key.
[Official implementation guide](https://docs.near.org/chain-abstraction/chain-signatures/implementation),
[key derivation](https://github.com/near/mpc/blob/3.18.0/crates/near-mpc-crypto-types/src/kdf.rs).

Read-only checks on November 10, 2026 found Solana available but Ethereum absent
from the advertised foreign-chain sets of `v1.signer` and
`v1.signer-prod.testnet`. **Public Ethereum inbound evidence remains an unresolved
integration requirement.** ECDSA signing for Ethereum does not imply support for
reading Ethereum. Recheck advertised chains, configured networks, code and keys
before selecting a deployment; do not substitute another chain or trusted backend
without changing the agreed trust model.
[Provider admission](https://github.com/near/mpc/blob/3.18.0/crates/contract/src/api/foreign_chain/support.rs).

The local MPC worker reads finalized NEAR commands and independently fetches
finalized native evidence before signing. Public operators' source-finality
settings need separate verification; their signature is not itself a NEAR
finality proof. [Operator configuration](https://github.com/near/mpc/blob/3.18.0/crates/node-config/src/lib.rs).

Two evidence details affect the contract design:

- Ethereum log selection uses the global RPC log index, not its position inside
  a receipt's log array. Emitter, topics, payload, successful execution and
  canonical finalized block must match.
- Solana AccountState can describe a read after the requested transaction's slot.
  It therefore cannot prove that arbitrary mutable contents came from that
  transaction. Apodect uses immutable receipts and checks their owner and
  vault-derived PDA.

See the official [EVM](https://github.com/near/mpc/blob/3.18.0/crates/foreign-chain-inspector/src/evm/inspector.rs)
and [Solana](https://github.com/near/mpc/blob/3.18.0/crates/foreign-chain-inspector/src/svm/inspector.rs)
inspectors. Native NEAR evidence uses vault callbacks and needs no foreign provider.

## Liquidity delivery

The application requests exact output and reserves the complete quoted input,
including fees and buffer. It binds the quote's deposit address/memo, recipients,
amounts and expiry before Apply. Provider status reports progress; only attributed
native receipts authorize accounting changes.

The Rust public adapter verifies the provider's Ed25519 quote signature, binds
the selected asset IDs to native addresses and six-decimal tokens in the
supported-token catalog, and admits only the provider's mainnet networks.
Quotes requiring a dispatch memo are rejected by the current vault integration.
These checks do not prove public route execution or MPC admission.
[Quote verification](https://docs.near-intents.org/integration/distribution-channels/1click-api/verify-quote-signature),
[supported tokens](https://docs.near-intents.org/api-reference/oneclick/get-supported-tokens).

The local provider has a separate private store, discovers finalized coordinator
routes and independently proves source dispatch before spending its finite token
inventory. It communicates with the application through HTTP quote/status APIs;
reading the relayer's database is not part of that interface.

NEAR uses keyless per-route receivers for delivery and refunds because ordinary
FT transfers do not provide the attributed callback required by Project accounting.
The receivers retain partial delivery and sweep only actual tokens to their bound
Project. Public integration must validate supported recipients, refund behavior,
storage registration, assets and fees against the selected provider.
[1Click swap types](https://docs.near-intents.org/integration/distribution-channels/1click-api/swap-types),
[quote API](https://docs.near-intents.org/api-reference/oneclick/request-a-swap-quote).

1Click does not provide a testnet deployment; the local Intents Verifier tools are
a different integration layer. This is why the local stack uses a delivery
simulator. [Official FAQ](https://docs.near-intents.org/resources/faqs).

## Before public deployment

Resolve the Ethereum evidence route, connect and verify live signing/delivery
providers, and configure actual tokens, domains, derived keys and finality.
Provision native gas/rent/storage and retained evidence history. Establish signer
and program-authority custody, recovery procedures and review of the exact
artifacts/configuration. Local acceptance tests alone do not establish these facts.
