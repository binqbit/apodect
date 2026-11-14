import { actionCreators } from "@near-js/transactions";
import { Buffer } from "buffer";
import type { InjectedWallet } from "@near-wallet-selector/core";
import type { PublicConfig, UnsignedIntent } from "../api";
import {
  type ConnectedWallet,
  type WalletOption,
  TransactionFailed,
  TransactionNotSubmitted,
  decodeBase64,
  requireActor,
  rpc,
  runTransactions,
  validateChallenge,
} from "../wallets";

export async function discoverNear(
  config: PublicConfig,
): Promise<WalletOption[]> {
  if (!["mainnet", "testnet"].includes(config.near.network_id)) return [];
  const { default: myNearWalletIcon } =
    await import("@near-wallet-selector/my-near-wallet/assets/my-near-wallet-icon.png?url");
  return [
    {
      id: "near:my-near-wallet",
      chain: "near",
      name: "MyNearWallet",
      icon: myNearWalletIcon,
      connect: async () => {
        // MyNearWallet's browser connector uses Buffer for its wire encoding.
        // Supply that specific polyfill before importing the connector.
        if (
          !(globalThis as typeof globalThis & { Buffer?: typeof Buffer }).Buffer
        ) {
          (
            globalThis as typeof globalThis & { Buffer?: typeof Buffer }
          ).Buffer = Buffer;
        }
        const [{ setupWalletSelector }, { setupMyNearWallet }] =
          await Promise.all([
            import("@near-wallet-selector/core"),
            import("@near-wallet-selector/my-near-wallet"),
          ]);
        const network = config.near.network_id as "mainnet" | "testnet";
        const selector = await setupWalletSelector({
          network: {
            networkId: network,
            nodeUrl: `${location.origin}/api/rpc/near`,
            helperUrl:
              network === "mainnet"
                ? "https://helper.mainnet.near.org"
                : "https://helper.testnet.near.org",
            explorerUrl: "",
            indexerUrl: "",
          },
          modules: [setupMyNearWallet()],
          debug: false,
        });
        const wallet = await selector.wallet<InjectedWallet>("my-near-wallet");
        const accounts = await wallet.signIn({});
        const actor = accounts[0]?.accountId;
        if (!actor) throw new Error("The wallet did not share a NEAR account.");
        const callbackUrl = `${location.origin}/wallet/callback`;
        return {
          chain: "near",
          address: actor,
          name: "MyNearWallet",
          callbackUrl,
          async signChallenge(challenge) {
            validateChallenge(challenge, "near", actor);
            if (challenge.callback_url !== callbackUrl)
              throw new Error(
                "The NEAR signing callback does not match the authentication request.",
              );
            if (!wallet.signMessage)
              throw new Error("This wallet cannot sign NEP-413 messages.");
            const signed = await wallet.signMessage({
              message: challenge.message,
              nonce: Buffer.from(challenge.nonce),
              recipient: challenge.recipient,
              callbackUrl,
            });
            if (!signed)
              throw new Error("The wallet did not return a message signature.");
            requireActor(signed.accountId, actor, "near");
            return {
              challenge: challenge.id,
              signature: signed.signature,
              public_key: signed.publicKey,
            };
          },
          async execute(intent, progress) {
            if (intent.chain !== "near")
              throw new Error("This action requires a NEAR wallet.");
            requireActor(actor, intent.actor, "near");
            await verifyNetwork(intent, config);
            return runTransactions({
              chain: "near",
              actor,
              config,
              total: intent.transactions.length,
              intent,
              submission: "wallet",
              progress,
              submit: async (index, _record, beginSubmission) => {
                const current = await wallet.getAccounts();
                if (!current.some((account) => account.accountId === actor))
                  throw new TransactionNotSubmitted(
                    "The selected NEAR account changed. Reconnect it before signing.",
                  );
                const call = intent.transactions[index];
                const keyList = await rpc<{
                  keys: {
                    public_key: string;
                    access_key: { nonce: number; permission: unknown };
                  }[];
                }>("near", "query", {
                  request_type: "view_access_key_list",
                  account_id: actor,
                  finality: "final",
                });
                const keys = keyList.keys.filter(
                  (entry) => entry.access_key.permission === "FullAccess",
                );
                if (
                  !keys.length ||
                  keys.some(
                    (entry) => !Number.isSafeInteger(entry.access_key.nonce),
                  )
                )
                  throw new TransactionNotSubmitted(
                    "The NEAR signing key nonce could not be verified.",
                  );
                beginSubmission(
                  Object.fromEntries(
                    keys.map((entry) => [
                      entry.public_key,
                      String(entry.access_key.nonce),
                    ]),
                  ),
                );
                // This adapter receives native actions. Passing parsed JSON here would
                // round coordinator integers outside JavaScript's exact number range.
                const outcomes = await wallet.signAndSendTransactions({
                  transactions: [
                    {
                      signerId: actor,
                      receiverId: call.receiver_id,
                      actions: [nearAction(call)],
                    },
                  ],
                });
                const outcome = outcomes?.[0];
                if (!outcome?.transaction?.hash)
                  throw new Error(
                    "The wallet did not return a transaction hash. Check its activity before trying again.",
                  );
                return outcome.transaction.hash;
              },
            });
          },
        } satisfies ConnectedWallet;
      },
    },
  ];
}

export function nearAction(
  call: Extract<UnsignedIntent, { chain: "near" }>["transactions"][number],
) {
  return actionCreators.functionCall(
    call.method_name,
    decodeBase64(call.args_base64),
    BigInt(call.gas),
    BigInt(call.deposit),
  );
}

export async function verifyNetwork(
  intent: Extract<UnsignedIntent, { chain: "near" }>,
  config: PublicConfig,
): Promise<void> {
  const actual = await rpc<Record<string, unknown>>(
    "near",
    "genesis_config",
    {},
  );
  for (const key of [
    "chain_id",
    "genesis_time",
    "genesis_height",
    "validators",
  ] as const) {
    if (
      JSON.stringify(actual[key]) !== JSON.stringify(intent.network[key]) ||
      JSON.stringify(actual[key]) !== JSON.stringify(config.near.genesis[key])
    )
      throw new Error("The NEAR network does not match this deployment.");
  }
}

export async function isFinalized(
  hash: string,
  intent: Extract<UnsignedIntent, { chain: "near" }>,
): Promise<boolean> {
  type ExecutionStatus =
    | string
    | { SuccessValue?: string; SuccessReceiptId?: string; Failure?: unknown };
  type Receipt = { outcome: { status: ExecutionStatus } };
  let outcome: {
    final_execution_status?: string;
    status: ExecutionStatus;
    transaction_outcome?: Receipt;
    receipts_outcome?: Receipt[];
  };
  try {
    outcome = await rpc("near", "tx", {
      tx_hash: hash,
      sender_account_id: intent.actor,
      wait_until: "FINAL",
    });
  } catch (error) {
    if (
      error instanceof Error &&
      /UNKNOWN_TRANSACTION|doesn't exist|does not exist/i.test(error.message)
    )
      return false;
    throw error;
  }
  if (outcome.final_execution_status !== "FINAL") return false;
  const statuses = [
    outcome.status,
    outcome.transaction_outcome?.outcome.status,
    ...(outcome.receipts_outcome ?? []).map(
      (receipt) => receipt.outcome.status,
    ),
  ];
  // A successful resolve/refund callback can hide a failed intermediate call.
  // Financial pipelines must inspect every receipt before requesting step two.
  if (
    statuses.some(
      (status) => status && typeof status === "object" && "Failure" in status,
    )
  )
    throw new TransactionFailed(
      "The NEAR transaction finalized with a failure. No later step was submitted.",
    );
  if (
    !Array.isArray(outcome.receipts_outcome) ||
    statuses.some(
      (status) =>
        !status ||
        typeof status !== "object" ||
        !("SuccessValue" in status || "SuccessReceiptId" in status),
    )
  )
    throw new Error(
      "The finalized NEAR response contains an unresolved receipt. Check its status before continuing.",
    );
  if (typeof outcome.status !== "object" || !("SuccessValue" in outcome.status))
    return false;
  const call = intent.transactions[0];
  if (call.method_name === "ft_transfer_call") {
    // NEP-141 can finalize successfully while refunding the entire transfer.
    // Its result is the amount actually accepted, not a promise of credit.
    const requested: unknown = JSON.parse(
      new TextDecoder().decode(decodeBase64(call.args_base64)),
    ).amount;
    const accepted: unknown = JSON.parse(
      new TextDecoder().decode(decodeBase64(outcome.status.SuccessValue!)),
    );
    if (
      typeof requested !== "string" ||
      typeof accepted !== "string" ||
      !/^\d+$/.test(requested) ||
      !/^\d+$/.test(accepted) ||
      BigInt(accepted) > BigInt(requested)
    )
      throw new Error(
        "The finalized NEAR token transfer returned an unverifiable accepted amount. Check its outcome before continuing.",
      );
    if (BigInt(accepted) !== BigInt(requested))
      throw new TransactionFailed(
        "The NEAR token transfer finalized, but the project did not accept the full amount. Check the refund and confirmed balances before preparing another transfer.",
      );
  }
  return true;
}

export async function verifyTransaction(
  hash: string,
  intent: Extract<UnsignedIntent, { chain: "near" }>,
  nonces?: Record<string, string>,
): Promise<void> {
  const outcome = await rpc<{
    transaction: {
      signer_id: string;
      public_key: string;
      nonce: number;
      receiver_id: string;
      actions: {
        FunctionCall?: {
          method_name: string;
          args: string;
          gas: number;
          deposit: string;
        };
      }[];
    };
  }>("near", "tx", {
    tx_hash: hash,
    sender_account_id: intent.actor,
    wait_until: "NONE",
  });
  const actual = outcome.transaction;
  const call = actual.actions[0]?.FunctionCall;
  const expected = intent.transactions[0];
  if (
    actual.signer_id !== intent.actor ||
    actual.receiver_id !== expected.receiver_id ||
    actual.actions.length !== 1 ||
    !call ||
    call.method_name !== expected.method_name ||
    call.args !== expected.args_base64 ||
    BigInt(call.gas) !== BigInt(expected.gas) ||
    BigInt(call.deposit) !== BigInt(expected.deposit)
  )
    throw new Error(
      "This transaction does not match the pending NEAR action. Its signer, receiver and function call must match.",
    );
  const nonce = nonces?.[actual.public_key];
  if (
    nonce === undefined ||
    !Number.isSafeInteger(actual.nonce) ||
    BigInt(actual.nonce) <= BigInt(nonce)
  )
    throw new Error(
      "This transaction predates the pending request or uses a different signing key.",
    );
}
