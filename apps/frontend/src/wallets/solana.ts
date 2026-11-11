import { getWallets } from "@wallet-standard/app";
import type { Wallet, WalletAccount } from "@wallet-standard/base";
import type { StandardConnectFeature } from "@wallet-standard/features";
import type {
  SolanaSignMessageFeature,
  SolanaSignTransactionFeature,
} from "@solana/wallet-standard-features";
import {
  AccountRole,
  address,
  appendTransactionMessageInstructions,
  blockhash,
  compileTransaction,
  createTransactionMessage,
  getSignatureFromTransaction,
  getTransactionDecoder,
  getTransactionEncoder,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
} from "@solana/kit";
import bs58 from "bs58";
import type { PublicConfig, UnsignedIntent } from "../api";
import {
  type ConnectedWallet,
  type WalletOption,
  TransactionFailed,
  decodeBase64,
  encodeBase64,
  requireActor,
  rpc,
  runTransactions,
  validateChallenge,
} from "../wallets";

type SupportedWallet = Wallet & {
  features: StandardConnectFeature &
    SolanaSignMessageFeature &
    SolanaSignTransactionFeature;
};

export async function discoverSolana(
  config: PublicConfig,
): Promise<WalletOption[]> {
  return getWallets()
    .get()
    .filter((wallet) =>
      [
        "standard:connect",
        "solana:signMessage",
        "solana:signTransaction",
      ].every((feature) => feature in wallet.features),
    )
    .map((wallet, index) => ({
      id: `solana:${wallet.name}:${index}`,
      name: wallet.name,
      chain: "solana",
      connect: async () => {
        const supported = wallet as SupportedWallet;
        const { accounts } =
          await supported.features["standard:connect"].connect();
        const account = accounts.find((item) =>
          item.chains.some((chain) => chain.startsWith("solana:")),
        );
        if (!account)
          throw new Error("The wallet did not share a Solana account.");
        return standardConnection(supported, account, config);
      },
    }));
}

function standardConnection(
  wallet: SupportedWallet,
  account: WalletAccount,
  config: PublicConfig,
): ConnectedWallet {
  const assertAccount = () => {
    if (!wallet.accounts.some((current) => current.address === account.address))
      throw new Error(
        "The selected Solana account changed. Reconnect it before signing.",
      );
  };
  return solanaConnection(account.address, wallet.name, config, {
    async message(message) {
      assertAccount();
      const [signed] = await wallet.features["solana:signMessage"].signMessage({
        account,
        message,
      });
      if (!equalBytes(signed.signedMessage, message))
        throw new Error(
          "This wallet changed the sign-in message. Use a wallet that supports signing the exact message.",
        );
      return signed.signature;
    },
    async transaction(transaction) {
      assertAccount();
      if (
        !wallet.features[
          "solana:signTransaction"
        ].supportedTransactionVersions.includes(0)
      )
        throw new Error(
          "This wallet cannot sign the required Solana transaction version.",
        );
      const [signed] = await wallet.features[
        "solana:signTransaction"
      ].signTransaction({ account, transaction });
      return signed.signedTransaction;
    },
  });
}

export function solanaConnection(
  actor: string,
  name: string,
  config: PublicConfig,
  signer: {
    message: (message: Uint8Array) => Promise<Uint8Array>;
    transaction: (transaction: Uint8Array) => Promise<Uint8Array>;
  },
): ConnectedWallet {
  return {
    chain: "solana",
    address: actor,
    name,
    async signChallenge(challenge) {
      validateChallenge(challenge, "solana", actor);
      return {
        challenge: challenge.id,
        signature: bs58.encode(
          await signer.message(new TextEncoder().encode(challenge.message)),
        ),
      };
    },
    async execute(intent, progress) {
      if (intent.chain !== "solana")
        throw new Error("This action requires a Solana wallet.");
      requireActor(actor, intent.actor, "solana");
      return runTransactions({
        chain: "solana",
        actor,
        config,
        total: 1,
        intent,
        submission: "signed",
        progress,
        submit: async (_index, record) => {
          const genesis = await rpc<string>("solana", "getGenesisHash", []);
          if (
            genesis !== config.solana.genesis_hash ||
            genesis !== intent.genesis_hash
          )
            throw new Error(
              "The Solana network does not match this deployment.",
            );
          const transaction = await buildTransaction(intent);
          const signed = await signer.transaction(transaction);
          const decoder = getTransactionDecoder();
          const decoded = decoder.decode(signed);
          if (
            !equalBytes(
              decoded.messageBytes,
              decoder.decode(transaction).messageBytes,
            )
          )
            throw new Error(
              "The wallet changed the prepared Solana transaction. Nothing was submitted.",
            );
          const hash = getSignatureFromTransaction(decoded);
          record(hash);
          const submitted = await rpc<string>("solana", "sendTransaction", [
            encodeBase64(signed),
            {
              encoding: "base64",
              preflightCommitment: "confirmed",
              skipPreflight: false,
              maxRetries: 0,
            },
          ]);
          if (submitted !== hash)
            throw new Error(
              "The RPC returned a different transaction identity. Check the original transaction before retrying.",
            );
          return hash;
        },
      });
    },
  };
}

export async function buildTransaction(
  intent: Extract<UnsignedIntent, { chain: "solana" }>,
): Promise<Uint8Array> {
  const recent = await rpc<{
    value: { blockhash: string; lastValidBlockHeight: number };
  }>("solana", "getLatestBlockhash", [{ commitment: "finalized" }]);
  const payer = setTransactionMessageFeePayer(
    address(intent.actor),
    createTransactionMessage({ version: 0 }),
  );
  const lifetime = setTransactionMessageLifetimeUsingBlockhash(
    {
      blockhash: blockhash(recent.value.blockhash),
      lastValidBlockHeight: BigInt(recent.value.lastValidBlockHeight),
    },
    payer,
  );
  const message = appendTransactionMessageInstructions(
    intent.instructions.map((instruction) => ({
      programAddress: address(instruction.program_id),
      accounts: instruction.accounts.map((account) => ({
        address: address(account.address),
        role: account.signer
          ? account.writable
            ? AccountRole.WRITABLE_SIGNER
            : AccountRole.READONLY_SIGNER
          : account.writable
            ? AccountRole.WRITABLE
            : AccountRole.READONLY,
      })),
      data: decodeBase64(instruction.data_base64),
    })),
    lifetime,
  );
  return new Uint8Array(
    getTransactionEncoder().encode(compileTransaction(message)),
  );
}

function equalBytes(
  left: ArrayLike<number>,
  right: ArrayLike<number>,
): boolean {
  return (
    left.length === right.length &&
    Array.from(left).every((value, index) => value === right[index])
  );
}

export async function isFinalized(hash: string): Promise<boolean> {
  const result = await rpc<{
    value: ({ confirmationStatus: string; err: unknown } | null)[];
  }>("solana", "getSignatureStatuses", [
    [hash],
    { searchTransactionHistory: true },
  ]);
  const receipt = result.value[0];
  if (!receipt || receipt.confirmationStatus !== "finalized") return false;
  if (receipt.err)
    throw new TransactionFailed(
      "The Solana transaction finalized with a failure. No later step was submitted.",
    );
  return true;
}
