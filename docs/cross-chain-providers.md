# Provider integration

The local stack runs real contracts and test-token transfers. External Chain
Signatures, Read MPC and 1Click-style liquidity delivery use local simulators.
Public adapters are implemented, but public execution, refunds and provider
availability require separate validation.

## Chain Signatures and Read MPC

The interface targets the reviewed [NEAR MPC 3.18.0](https://github.com/near/mpc/releases/tag/3.18.0)
API. The coordinator requests signatures only for stored, authorized commands.
Command signing and foreign evidence use separate keys:

- **Signing:** domain 0, derived for the coordinator account and its configured
  path. Deriving for the worker account produces the wrong key.
- **Foreign verification:** the domain-3 root key pinned in the coordinator.

Public admission checks both keys, active key-domain purposes, the MPC gas budget
and advertised foreign verification for Ethereum **and** Solana. Missing support
blocks public startup; command signing alone is insufficient. The
[admission implementation](../apps/backend/src/providers.rs) defines the checks.
Operator network/finality settings and request liveness still need verification.

Evidence must match the registered application and finalized native outcome:

- Ethereum uses the global RPC log index and verifies the vault, event and payload.
- Solana uses immutable receipt accounts, their program owner and vault-derived
  addresses. A later read of a mutable account cannot prove its transaction-time
  contents.
- NEAR uses authenticated vault callbacks without a foreign provider.

The [foreign evidence decoder](../contracts/near/protocol/src/foreign.rs) pins the
accepted MPC types. See the official
[key derivation guide](https://docs.near.org/chain-abstraction/chain-signatures/implementation)
when initializing the coordinator.

## Liquidity delivery

Apply uses exact-output quotes. The complete quoted input, including fees and
buffer, is reserved before dispatch. Provider status tracks progress; attributed
native receipts establish delivery or refund.

The public adapter verifies the
[1Click quote signature](https://docs.near-intents.org/integration/distribution-channels/1click-api/verify-quote-signature)
and checks selected assets against the supported-token catalog. It accepts only
native six-decimal USDC on the configured mainnets and rejects quotes requiring
a dispatch memo. These restrictions are enforced by the
[liquidity service](../apps/backend/src/liquidity.rs).

Receiver identities matter when configuring routes:

- On Solana, quote recipients are **owners**, whose canonical USDC associated
  token accounts receive delivery. A route PDA is the receiving owner; supplying
  its token account as the recipient selects a different associated account.
- On NEAR, keyless per-route receivers attribute delivery and refunds to the
  correct Project. They retain partial output and sweep actual received tokens.
  Ordinary FT transfers cannot replace this accounting callback.

The local provider has its own store and finite token inventory. It observes
finalized routes and proves source dispatch independently, then exposes HTTP
quote/status APIs. It does not read the application's database.

## Public deployment setup

1. Copy [deployment.example.json](deployment.example.json) to a private configuration
   such as `.secrets/deployment.json`. Fill in the RPC endpoints, coordinator,
   Project ID and native vault addresses. Keep `local_simulator` false. The example
   contains mainnet network and USDC identities; admission rechecks the assets.
   Set `ethereum_start_block` to the exact vault creation block once deployed.
2. Build and deploy matching contracts and clients. Solana's program ID is compiled
   into the program and backend: select a program keypair, update `declare_id!`
   and `Anchor.toml`, then build both from that source. Choose the upgrade authority
   explicitly and retain release artifacts and deployment transactions.
3. Initialize the coordinator with the signing and Read MPC keys above. Before
   binding fresh vaults, derive their domains from the completed identities:

   ```sh
   APODECT_CONFIG=.secrets/deployment.json apodect --deployment-domains
   ```

   Use the returned values in the configuration, coordinator registration and vault
   bindings. Domain placeholders may remain until this step; derivation needs no
   RPC, database or signing keys. Incorrect immutable bindings require a new
   deployment.
4. Provision RPC access and dedicated gas payers as described below, then run
   `apodect --check-deployment` with `APODECT_CONFIG` set. It reads bindings, keys,
   network identities and provider capabilities without loading relay credentials,
   using the database or submitting transactions. The
   [live host guide](application-development.md#live-host) covers Compose and secrets,
   including an optional liquidity API key.

RPCs must retain finalized transactions and receipts while operations are unsettled.
Ethereum needs its genesis block, logs from vault creation and archive-state
`eth_getCode` at the creation block and its predecessor. Solana needs transaction
history and immutable receipt accounts. A pruned response cannot prove non-execution.

Gas payers need ETH, SOL and NEAR for transactions, rent, storage and route-receiver
deployment. Owner, manager and agent keys stay in their own wallets. Verify NEAR
token storage registration and provider support for the configured delivery/refund
receivers. A passing admission report does not prove a completed transfer: validate
a bounded funded route, delivery, refunds and restart recovery before using user
funds.
