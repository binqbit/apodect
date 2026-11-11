import { KeyPairSigner } from "@near-js/signers";
import { createTransaction, encodeTransaction } from "@near-js/transactions";
import {
  createKeyPairSignerFromBytes,
  getTransactionDecoder,
  getTransactionEncoder,
  signTransaction,
} from "@solana/kit";
import { privateKeyToAccount } from "viem/accounts";
import { keccak256, type Hex, type Address } from "viem";
import bs58 from "bs58";
import type { PublicConfig } from "../api";
import {
  type ConnectedWallet,
  encodeBase64,
  requireActor,
  rpc,
  runTransactions,
  validateChallenge,
} from "../wallets";
import { ethereumConnection, type EthereumProvider } from "./ethereum";
import { solanaConnection } from "./solana";
import { nearAction, verifyNetwork } from "./near";

/** Never store imported keys in browser storage or send them to the API. */
export async function importWallet(
  input: unknown,
  config: PublicConfig,
): Promise<ConnectedWallet> {
  if (
    !input ||
    typeof input !== "object" ||
    !("chain" in input) ||
    !("address" in input) ||
    typeof input.address !== "string"
  )
    throw new Error("Choose an Apodect development wallet export.");
  const data = input as {
    chain: string;
    address: string;
    private_key?: unknown;
    secret_key?: unknown;
  };
  if (data.chain === "solana") {
    if (
      !Array.isArray(data.secret_key) ||
      data.secret_key.length !== 64 ||
      !data.secret_key.every(
        (byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255,
      )
    )
      throw new Error("The Solana development key must contain 64 bytes.");
    const signer = await createKeyPairSignerFromBytes(
      new Uint8Array(data.secret_key),
    );
    requireActor(signer.address, data.address, "solana");
    return solanaConnection(
      signer.address,
      "Local development wallet",
      config,
      {
        message: async (message) =>
          new Uint8Array(
            await crypto.subtle.sign(
              "Ed25519",
              signer.keyPair.privateKey,
              new Uint8Array(message),
            ),
          ),
        transaction: async (bytes) =>
          new Uint8Array(
            getTransactionEncoder().encode(
              await signTransaction(
                [signer.keyPair],
                getTransactionDecoder().decode(bytes),
              ),
            ),
          ),
      },
    );
  }
  if (typeof data.private_key !== "string")
    throw new Error("The development wallet file is missing its key.");
  if (data.chain === "ethereum")
    return importEthereum(data.address, data.private_key, config);
  if (data.chain === "near")
    return importNear(data.address, data.private_key, config);
  throw new Error("This development wallet uses an unsupported network.");
}

function importEthereum(
  address: string,
  secret: string,
  config: PublicConfig,
): ConnectedWallet {
  if (!/^0x[0-9a-fA-F]{64}$/.test(secret))
    throw new Error("The Ethereum development key is invalid.");
  const account = privateKeyToAccount(secret as Hex);
  requireActor(account.address, address, "ethereum");
  const provider: EthereumProvider = {
    request: async ({ method, params = [] }) => {
      if (method === "eth_accounts") return [account.address];
      if (method === "personal_sign")
        return account.signMessage({ message: { raw: params[0] as Hex } });
      return rpc("ethereum", method, params);
    },
    sendTransaction: async (transaction, record) => {
      const [gas, gasPrice] = await Promise.all([
        rpc<string>("ethereum", "eth_estimateGas", [transaction]),
        rpc<string>("ethereum", "eth_gasPrice", []),
      ]);
      const nonce = Number(BigInt(transaction.nonce));
      if (!Number.isSafeInteger(nonce))
        throw new Error(
          "The transaction nonce exceeds this wallet's supported range.",
        );
      const signed = await account.signTransaction({
        type: "legacy",
        chainId: Number(BigInt(transaction.chainId)),
        nonce,
        to: transaction.to as Address,
        data: transaction.data as Hex,
        value: BigInt(transaction.value),
        gas: BigInt(gas),
        gasPrice: BigInt(gasPrice),
      });
      const hash = keccak256(signed);
      record(hash);
      const returned = await rpc<string>("ethereum", "eth_sendRawTransaction", [
        signed,
      ]);
      if (returned !== hash)
        throw new Error(
          "The RPC returned a different transaction identity. Check the original transaction before retrying.",
        );
      return hash;
    },
  };
  return ethereumConnection(
    provider,
    account.address,
    "Local development wallet",
    config,
  );
}

function importNear(
  actor: string,
  secret: string,
  config: PublicConfig,
): ConnectedWallet {
  if (!secret.startsWith("ed25519:"))
    throw new Error("Import a NEAR ed25519 development key.");
  let signer: KeyPairSigner;
  try {
    signer = KeyPairSigner.fromSecretKey(secret as `ed25519:${string}`);
  } catch {
    throw new Error("The NEAR development key is invalid.");
  }
  return {
    chain: "near",
    address: actor,
    name: "Local development wallet",
    async signChallenge(challenge) {
      validateChallenge(challenge, "near", actor);
      const signed = await signer.signNep413Message(
        challenge.message,
        actor,
        challenge.recipient,
        new Uint8Array(challenge.nonce),
        challenge.callback_url,
      );
      return {
        challenge: challenge.id,
        public_key: signed.publicKey.toString(),
        signature: encodeBase64(signed.signature),
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
        submission: "signed",
        progress,
        submit: async (index, record) => {
          const publicKey = await signer.getPublicKey();
          const key = await rpc<{
            nonce: number;
            permission: unknown;
            block_hash: string;
          }>("near", "query", {
            request_type: "view_access_key",
            account_id: actor,
            public_key: publicKey.toString(),
            finality: "final",
          });
          if (key.permission !== "FullAccess")
            throw new Error(
              "The imported key is not a full-access key for this NEAR account.",
            );
          if (!Number.isSafeInteger(key.nonce))
            throw new Error(
              "The NEAR nonce exceeds this wallet's supported range.",
            );
          const call = intent.transactions[index];
          const transaction = createTransaction(
            actor,
            publicKey,
            call.receiver_id,
            BigInt(key.nonce) + 1n,
            [nearAction(call)],
            bs58.decode(key.block_hash),
          );
          const [hashBytes, signed] = await signer.signTransaction(transaction);
          const hash = bs58.encode(hashBytes);
          record(hash);
          const returned = await rpc<string>("near", "broadcast_tx_async", [
            encodeBase64(encodeTransaction(signed)),
          ]);
          if (returned !== hash)
            throw new Error(
              "The RPC returned a different transaction identity. Check the original transaction before retrying.",
            );
          return hash;
        },
      });
    },
  };
}
