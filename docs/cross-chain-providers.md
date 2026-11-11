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

Public admission requires foreign reads for the configured Ethereum and Solana
networks, in addition to command signing. ECDSA signing for Ethereum does not imply
support for reading Ethereum. The deployment check verifies the selected service's
advertised capabilities and keys; unavailable capabilities block public startup.
Do not substitute another chain or trusted backend for missing Read MPC support
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
the selected asset IDs to the native six-decimal USDC addresses in the
supported-token catalog, and admits only the provider's mainnet networks.
Quotes requiring a dispatch memo are rejected by the current vault integration.
These checks do not prove public route execution or MPC admission.
[Quote verification](https://docs.near-intents.org/integration/distribution-channels/1click-api/verify-quote-signature),
[supported tokens](https://docs.near-intents.org/api-reference/oneclick/get-supported-tokens).

Solana quote addresses identify the receiving owner, whose canonical USDC
associated token account receives the tokens. Each route uses its route PDA as
that owner; source dispatch likewise binds the provider's raw owner address and
creates its USDC account when needed. Passing a token account as the quote's
recipient would send funds to a different associated account.

This custody layout and the command's ATA-program account require matching
program and client releases. Redeploy older development ledgers before testing
these flows; their previous route accounts are not migrated automatically.

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

## Register an existing public deployment

Start with [deployment.example.json](deployment.example.json). It contains mainnet
genesis identities and native USDC addresses from
[Circle](https://developers.circle.com/stablecoins/usdc-contract-addresses), together
with the corresponding 1Click asset IDs. Replace every `REPLACE_WITH_...` value
with your RPC endpoints and deployed contract identities. Set
`ethereum_start_block` to the exact Project creation block instead of the example's
zero. Keep `local_simulator` false; admission rechecks the token catalog.

The deployment must already have its coordinator and three vaults bound to the
same domains and command key. Derive that key for the coordinator account and its
configured path, and verify the separate Read MPC root key. Record the exact
release artifacts and retain their native deployment transactions. An incorrect
immutable binding requires a new deployment.

Before binding, fill in the coordinator, Project ID and native vault/token
addresses in the configuration. Domain placeholders may remain at this step.
Run `APODECT_CONFIG=.secrets/deployment.json apodect --deployment-domains` and use
the returned domains in the configuration, coordinator registration and vault
bindings. The calculation uses canonical network identities and needs no RPC,
database or keys. Public admission rejects domains derived for another deployment.

Solana's program ID is compiled into both the program and backend. For a public
release, select a program keypair under your custody, update `declare_id!` and
`Anchor.toml`, then rebuild the SBF program and backend from that same source.
Deploy through the official [Solana deployment tools](https://solana.com/docs/programs/deploying)
with an explicit upgrade-authority policy. Preloading the development address
into a local validator does not deploy that address publicly.

Use RPC services that retain finalized transactions and receipts for the lifetime
of unsettled operations. Ethereum also needs its genesis block and logs from the
Project creation block. The creation-block check needs historical `eth_getCode`
at that block and its predecessor, so use an Ethereum endpoint with archive-state
access. Solana must retain transaction history as well as the
immutable receipt accounts. A pruned response cannot establish non-execution.

Provision dedicated gas-payer accounts with ETH, SOL and NEAR, including SOL rent,
NEAR storage and route-receiver deployment costs. Keep owner/manager credentials
in their own wallets. Confirm NEAR token storage registration for the Project and
the provider's support for its delivery/refund receivers. Provider fees, source
finality and authority custody need operational verification.

Run `apodect --check-deployment` against the completed configuration before
starting services; the [live host guide](application-development.md#live-host)
shows the Compose command. It is a read-only, database-free check and does not
load relayer keys or submit transactions. A successful check admits the configured
interfaces; it does not establish a successful public transfer. Validate a bounded
funded route, its native delivery, refunds and restart recovery before using the
deployment for user funds.
