import assert from "node:assert/strict";
import { after, test } from "node:test";
import { KeyPair } from "@near-js/crypto";
import {
  createTransaction,
  decodeTransaction,
  encodeTransaction,
} from "@near-js/transactions";
import type { PublicConfig, UnsignedIntent } from "../src/api.ts";
import {
  WalletExecutionError,
  runTransactions,
  walletTransactions,
  resolveWalletTransaction,
} from "../src/wallets.ts";
import { nearAction } from "../src/wallets/near.ts";

const originalFetch = globalThis.fetch;
const storage = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  },
});
after(() => {
  globalThis.fetch = originalFetch;
});

const config = {
  ethereum: { chain_id: "31337", genesis_hash: "genesis" },
} as PublicConfig;

const intent: UnsignedIntent = {
  chain: "ethereum",
  chain_id: 31337,
  actor: "owner",
  transactions: [
    { to: "recipient", data: "0x1234", value: "0" },
    { to: "recipient", data: "0x5678", value: "0" },
  ],
};

test("NEAR wallet wire actions preserve integers above JavaScript's exact range", () => {
  const args = '{"amount":18446744073709551615,"plan":"9223372036854775809"}';
  const call = nearAction({
    receiver_id: "vault.near",
    method_name: "apply",
    args_base64: Buffer.from(args).toString("base64"),
    gas: 100_000_000_000_000,
    deposit: "1000000000000000000000001",
  });
  const transaction = createTransaction(
    "owner.near",
    KeyPair.fromRandom("ed25519").getPublicKey(),
    "vault.near",
    1n,
    [call],
    new Uint8Array(32),
  );
  const wire = decodeTransaction(encodeTransaction(transaction));
  assert.equal(
    new TextDecoder().decode(
      new Uint8Array(wire.actions[0].functionCall!.args),
    ),
    args,
  );
  assert.equal(
    wire.actions[0].functionCall!.deposit,
    1000000000000000000000001n,
  );
});

test("wallet pipelines retain transaction identity and stop after unknown or finalized failure", async (context) => {
  for (const outcome of ["unknown", "failed"] as const) {
    await context.test(outcome, async () => {
      storage.clear();
      const submissions: number[] = [];
      globalThis.fetch = async (_url, request) => {
        if (outcome === "unknown")
          throw new Error("RPC connection lost after broadcast");
        const { method, params } = JSON.parse(request!.body as string);
        const result =
          method === "eth_getTransactionReceipt"
            ? { status: "0x0", blockNumber: "0x12", blockHash: "block" }
            : params[0] === "finalized"
              ? { number: "0x12" }
              : { hash: "block" };
        return Response.json({ jsonrpc: "2.0", result });
      };
      await assert.rejects(
        runTransactions({
          chain: "ethereum",
          actor: "owner",
          config,
          intent,
          submission: "wallet",
          total: 2,
          progress: () => {},
          submit: async (index, record) => {
            submissions.push(index);
            record("transaction-1");
            return "transaction-1";
          },
        }),
        (error) => {
          assert.ok(error instanceof WalletExecutionError);
          assert.equal(error.transactions[0].transactionId, "transaction-1");
          assert.equal(error.transactions[0].status, outcome);
          return true;
        },
      );
      assert.deepEqual(submissions, [0]);
      assert.equal(walletTransactions()[0].status, outcome);
      if (outcome === "unknown") {
        await assert.rejects(
          runTransactions({
            chain: "ethereum",
            actor: "owner",
            config,
            intent,
            submission: "wallet",
            total: 1,
            progress: () => {},
            submit: async () => {
              throw new Error("A duplicate submission must not be attempted");
            },
          }),
          /unresolved transaction/,
        );
      }
    });
  }
});

test("NEAR receipt failures stop the wallet pipeline even when the terminal callback succeeds", async () => {
  storage.clear();
  const network = {
    chain_id: "localnet",
    genesis_time: "2026-01-01T00:00:00Z",
    genesis_height: 0,
    validators: [],
  };
  const call = {
    receiver_id: "usdc.near",
    method_name: "ft_transfer_call",
    args_base64: Buffer.from("{}").toString("base64"),
    gas: 100_000_000_000_000,
    deposit: "1",
  };
  const nearIntent: UnsignedIntent = {
    chain: "near",
    actor: "owner.near",
    network,
    transactions: [call, { ...call, method_name: "followup" }],
  };
  const submissions: number[] = [];
  globalThis.fetch = async (_url, request) => {
    const { method } = JSON.parse(request!.body as string);
    assert.equal(method, "tx");
    return Response.json({
      jsonrpc: "2.0",
      result: {
        final_execution_status: "FINAL",
        status: { SuccessValue: Buffer.from("false").toString("base64") },
        transaction_outcome: {
          outcome: { status: { SuccessReceiptId: "initial-receipt" } },
        },
        receipts_outcome: [
          {
            outcome: {
              status: {
                Failure: {
                  ActionError: {
                    index: 0,
                    kind: {
                      FunctionCallError: {
                        ExecutionError: "transfer rejected",
                      },
                    },
                  },
                },
              },
            },
          },
          {
            outcome: {
              status: { SuccessValue: Buffer.from("false").toString("base64") },
            },
          },
        ],
      },
    });
  };
  await assert.rejects(
    runTransactions({
      chain: "near",
      actor: nearIntent.actor,
      config: { ...config, near: { network_id: "localnet", genesis: network } },
      intent: nearIntent,
      submission: "signed",
      total: 2,
      progress: () => {},
      submit: async (index, record) => {
        submissions.push(index);
        record("near-transfer");
        return "near-transfer";
      },
    }),
    (error) => {
      assert.ok(error instanceof WalletExecutionError);
      assert.equal(error.transactions[0].status, "failed");
      assert.equal(error.transactions[0].transactionId, "near-transfer");
      return true;
    },
  );
  assert.deepEqual(submissions, [0]);
  assert.equal(walletTransactions()[0].status, "failed");
});

test("lost wallet responses remain recoverable before a hash, while explicit rejection permits retry", async () => {
  storage.clear();
  await assert.rejects(
    runTransactions({
      chain: "ethereum",
      actor: "owner",
      config,
      intent,
      submission: "wallet",
      total: 1,
      progress: () => {},
      submit: async (_index, _record, beginSubmission) => {
        beginSubmission({ owner: "7" });
        const persisted = walletTransactions()[0];
        assert.equal(persisted.transactionId, null);
        assert.deepEqual(persisted.expected, {
          ...intent,
          transactions: [intent.transactions[0]],
        });
        throw new Error("Wallet response lost after submission");
      },
    }),
    WalletExecutionError,
  );
  const unresolved = walletTransactions()[0];
  assert.equal(unresolved.status, "unknown");
  let matches = false;
  globalThis.fetch = async (_url, request) => {
    const { method, params } = JSON.parse(request!.body as string);
    const result =
      method === "eth_getTransactionByHash"
        ? {
            from: "owner",
            to: "recipient",
            input: "0x1234",
            value: "0x0",
            nonce: matches ? "0x7" : "0x6",
          }
        : method === "eth_getTransactionReceipt"
          ? { status: "0x1", blockNumber: "0x12", blockHash: "block" }
          : params[0] === "finalized"
            ? { number: "0x12" }
            : { hash: "block" };
    return Response.json({ jsonrpc: "2.0", result });
  };
  const hash = `0x${"ab".repeat(32)}`;
  await assert.rejects(
    resolveWalletTransaction(unresolved, hash, config),
    /nonce/,
  );
  assert.equal(walletTransactions()[0].transactionId, null);
  matches = true;
  await resolveWalletTransaction(unresolved, hash, config);
  assert.equal(walletTransactions()[0].status, "finalized");

  await assert.rejects(
    runTransactions({
      chain: "ethereum",
      actor: "owner",
      config,
      intent,
      submission: "wallet",
      total: 1,
      progress: () => {},
      submit: async (_index, _record, beginSubmission) => {
        beginSubmission({ owner: "8" });
        throw Object.assign(new Error("User rejected the transaction"), {
          code: 4001,
        });
      },
    }),
    WalletExecutionError,
  );
  assert.equal(walletTransactions().at(-1)?.status, "failed");
});

test("each wallet transaction finalizes before the next signature is requested", async () => {
  storage.clear();
  const order: string[] = [];
  globalThis.fetch = async (_url, request) => {
    const { method, params } = JSON.parse(request!.body as string);
    if (method === "eth_getTransactionReceipt")
      order.push(`receipt:${params[0]}`);
    const result =
      method === "eth_getTransactionReceipt"
        ? { status: "0x1", blockNumber: "0x12", blockHash: "block" }
        : params[0] === "finalized"
          ? { number: "0x12" }
          : { hash: "block" };
    return Response.json({ jsonrpc: "2.0", result });
  };
  await runTransactions({
    chain: "ethereum",
    actor: "owner",
    config,
    intent,
    submission: "wallet",
    total: 2,
    progress: ({ stage, step }) => {
      if (stage === "finalized") order.push(`finalized:${step}`);
    },
    submit: async (index) => {
      order.push(`submit:${index + 1}`);
      return `tx-${index + 1}`;
    },
  });
  assert.deepEqual(order, [
    "submit:1",
    "receipt:tx-1",
    "finalized:1",
    "submit:2",
    "receipt:tx-2",
    "finalized:2",
  ]);
});
