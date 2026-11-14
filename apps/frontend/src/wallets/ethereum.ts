import type { PublicConfig, UnsignedIntent } from "../api";
import {
  type ConnectedWallet,
  type WalletOption,
  TransactionFailed,
  TransactionNotSubmitted,
  requireActor,
  rpc,
  runTransactions,
  validateChallenge,
} from "../wallets";

export interface EthereumProvider {
  request: (input: { method: string; params?: unknown[] }) => Promise<unknown>;
  sendTransaction?: (
    transaction: Record<string, string>,
    record: (hash: string) => void,
  ) => Promise<string>;
}

interface Announcement {
  info: { uuid: string; name: string; icon?: string };
  provider: EthereumProvider;
}
const providers = new Map<string, Announcement>();
let listening = false;

export async function discoverEthereum(
  config: PublicConfig,
): Promise<WalletOption[]> {
  if (!listening) {
    window.addEventListener("eip6963:announceProvider", (event) => {
      const announcement = (event as CustomEvent<Announcement>).detail;
      if (
        announcement?.info?.uuid &&
        typeof announcement.provider?.request === "function"
      )
        providers.set(announcement.info.uuid, announcement);
    });
    listening = true;
  }
  window.dispatchEvent(new Event("eip6963:requestProvider"));
  await new Promise((resolve) => setTimeout(resolve, 100));
  const legacy = (window as Window & { ethereum?: EthereumProvider }).ethereum;
  if (
    legacy &&
    ![...providers.values()].some((item) => item.provider === legacy)
  ) {
    providers.set("injected", {
      info: { uuid: "injected", name: "Ethereum wallet" },
      provider: legacy,
    });
  }
  return [...providers.values()].map(({ info, provider }) => ({
    id: `ethereum:${info.uuid}`,
    name: info.name,
    icon: info.icon,
    chain: "ethereum",
    connect: async () => {
      const accounts = (await provider.request({
        method: "eth_requestAccounts",
      })) as string[];
      if (!accounts[0]) throw new Error("The wallet did not share an account.");
      return ethereumConnection(provider, accounts[0], info.name, config);
    },
  }));
}

export function ethereumConnection(
  provider: EthereumProvider,
  address: string,
  name: string,
  config: PublicConfig,
): ConnectedWallet {
  const activeAccount = async () => {
    const accounts = (await provider.request({
      method: "eth_accounts",
    })) as string[];
    if (!accounts[0]) throw new Error("Reconnect your Ethereum wallet.");
    requireActor(accounts[0], address, "ethereum");
  };
  return {
    chain: "ethereum",
    address,
    name,
    async signChallenge(challenge) {
      validateChallenge(challenge, "ethereum", address);
      await activeAccount();
      const bytes = new TextEncoder().encode(challenge.message);
      const message = `0x${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
      const signature = (await provider.request({
        method: "personal_sign",
        params: [message, address],
      })) as string;
      return { challenge: challenge.id, signature };
    },
    async execute(intent, progress) {
      if (intent.chain !== "ethereum")
        throw new Error("This action requires a different wallet network.");
      requireActor(address, intent.actor, "ethereum");
      await verifyNetwork(provider, config, intent);
      return runTransactions({
        chain: "ethereum",
        actor: address,
        config,
        total: intent.transactions.length,
        intent,
        submission: provider.sendTransaction ? "signed" : "wallet",
        progress,
        submit: async (index, record, beginSubmission) => {
          let nonce: string;
          try {
            await activeAccount();
            await verifyNetwork(provider, config, intent);
            nonce = (await provider.request({
              method: "eth_getTransactionCount",
              params: [address, "pending"],
            })) as string;
          } catch (error) {
            throw new TransactionNotSubmitted(
              error instanceof Error
                ? error.message
                : "The wallet is not ready to submit this transaction.",
            );
          }
          const call = intent.transactions[index];
          const transaction = {
            from: address,
            to: call.to,
            data: call.data,
            value: `0x${BigInt(call.value).toString(16)}`,
            chainId: `0x${BigInt(intent.chain_id).toString(16)}`,
            nonce,
          };
          beginSubmission({
            [address.toLowerCase()]: BigInt(nonce).toString(),
          });
          return provider.sendTransaction
            ? provider.sendTransaction(transaction, record)
            : ((await provider.request({
                method: "eth_sendTransaction",
                params: [transaction],
              })) as string);
        },
      });
    },
  };
}

export async function verifyTransaction(
  hash: string,
  intent: Extract<UnsignedIntent, { chain: "ethereum" }>,
  nonces?: Record<string, string>,
): Promise<void> {
  if (!/^0x[0-9a-fA-F]{64}$/.test(hash))
    throw new Error("Enter a valid Ethereum transaction hash.");
  const transaction = await rpc<{
    from: string;
    to: string | null;
    input: string;
    value: string;
    nonce: string;
  } | null>("ethereum", "eth_getTransactionByHash", [hash]);
  if (!transaction)
    throw new Error(
      "This transaction has not been found on the configured Ethereum network.",
    );
  const expected = intent.transactions[0];
  if (
    transaction.from.toLowerCase() !== intent.actor.toLowerCase() ||
    transaction.to?.toLowerCase() !== expected.to.toLowerCase() ||
    transaction.input.toLowerCase() !== expected.data.toLowerCase() ||
    BigInt(transaction.value) !== BigInt(expected.value)
  )
    throw new Error(
      "This transaction does not match the pending wallet action. Its sender, destination, data and amount must match.",
    );
  const nonce = nonces?.[intent.actor.toLowerCase()];
  if (nonce === undefined || BigInt(transaction.nonce) !== BigInt(nonce))
    throw new Error(
      "This transaction does not use the nonce reserved for the pending action.",
    );
}

async function verifyNetwork(
  provider: EthereumProvider,
  config: PublicConfig,
  intent: Extract<UnsignedIntent, { chain: "ethereum" }>,
): Promise<void> {
  const chainId = BigInt(
    (await provider.request({ method: "eth_chainId" })) as string,
  );
  if (
    chainId !== BigInt(config.ethereum.chain_id) ||
    chainId !== BigInt(intent.chain_id)
  )
    throw new Error(
      `Switch your Ethereum wallet to chain ${config.ethereum.chain_id}.`,
    );
  const genesis = (await provider.request({
    method: "eth_getBlockByNumber",
    params: ["0x0", false],
  })) as { hash: string };
  if (genesis.hash.toLowerCase() !== config.ethereum.genesis_hash.toLowerCase())
    throw new Error(
      "Your Ethereum wallet is connected to a different deployment.",
    );
}

export async function isFinalized(hash: string): Promise<boolean> {
  const receipt = await rpc<{
    blockNumber: string;
    blockHash: string;
    status: string;
  } | null>("ethereum", "eth_getTransactionReceipt", [hash]);
  if (!receipt) return false;
  const block = await rpc<{ number: string }>(
    "ethereum",
    "eth_getBlockByNumber",
    ["finalized", false],
  );
  if (BigInt(block.number) < BigInt(receipt.blockNumber)) return false;
  const included = await rpc<{ hash: string }>(
    "ethereum",
    "eth_getBlockByNumber",
    [receipt.blockNumber, false],
  );
  if (included.hash !== receipt.blockHash) return false;
  if (BigInt(receipt.status) !== 1n)
    throw new TransactionFailed(
      "The Ethereum transaction finalized with a failure. No later step was submitted.",
    );
  return true;
}
