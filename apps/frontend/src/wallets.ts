import type {
  Chain,
  Challenge,
  PublicConfig,
  SignedChallenge,
  UnsignedIntent,
} from "./api";
import { api } from "./api";

export interface WalletOption {
  id: string;
  name: string;
  chain: Chain;
  connect: () => Promise<ConnectedWallet>;
}

export interface ConnectedWallet {
  chain: Chain;
  address: string;
  name: string;
  callbackUrl?: string;
  signChallenge: (challenge: Challenge) => Promise<SignedChallenge>;
  execute: (
    intent: UnsignedIntent,
    progress: (event: WalletProgress) => void,
  ) => Promise<WalletTransaction[]>;
}

export interface WalletProgress {
  stage:
    | "approval"
    | "submitted"
    | "confirming"
    | "finalized"
    | "failed"
    | "unknown";
  chain: Chain;
  step: number;
  total: number;
  transactionId?: string;
  message: string;
}

export interface WalletTransaction {
  id: string;
  chain: Chain;
  transactionId: string | null;
  actor: string;
  network: string;
  status: "submitted" | "finalized" | "failed" | "unknown";
  expected: UnsignedIntent;
  submission: "wallet" | "signed";
  nonceContext?: Record<string, string>;
}

export class WalletExecutionError extends Error {
  constructor(
    message: string,
    public transactions: WalletTransaction[],
  ) {
    super(message);
    this.name = "WalletExecutionError";
  }
}

export class TransactionFailed extends Error {}
export class TransactionNotSubmitted extends Error {}

export async function discoverWallets(
  chain: Chain,
  config: PublicConfig,
): Promise<WalletOption[]> {
  if (chain === "ethereum")
    return (await import("./wallets/ethereum")).discoverEthereum(config);
  if (chain === "solana")
    return (await import("./wallets/solana")).discoverSolana(config);
  return (await import("./wallets/near")).discoverNear(config);
}

export function connectWallet(
  option: WalletOption,
  _config: PublicConfig,
): Promise<ConnectedWallet> {
  return option.connect();
}

/** Development keys are imported explicitly and live only in the returned closure. */
export async function importDevelopmentWallet(
  file: File,
  config: PublicConfig,
): Promise<ConnectedWallet> {
  if (
    !config.local_simulator ||
    !["localhost", "127.0.0.1", "[::1]"].includes(location.hostname)
  ) {
    throw new Error(
      "Development wallets are available only on a local sandbox.",
    );
  }
  if (file.size > 16_384) throw new Error("The wallet file is too large.");
  try {
    const input: unknown = JSON.parse(await file.text());
    return await (
      await import("./wallets/development")
    ).importWallet(input, config);
  } catch {
    // Parser and crypto-library messages can echo input. Never display key bytes.
    throw new Error(
      "Choose a valid development wallet export whose address matches its key.",
    );
  }
}

export const rpc = api.rpc;

export function decodeBase64(value: string): Uint8Array {
  return Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
}

export function encodeBase64(value: Uint8Array): string {
  return btoa(Array.from(value, (byte) => String.fromCharCode(byte)).join(""));
}

export function sameAddress(
  actual: string,
  expected: string,
  chain: Chain,
): boolean {
  return chain === "ethereum"
    ? actual.toLowerCase() === expected.toLowerCase()
    : actual === expected;
}

export function requireActor(
  actual: string,
  expected: string,
  chain: Chain,
): void {
  if (!sameAddress(actual, expected, chain))
    throw new Error("Select the wallet that owns this action before signing.");
}

export function validateChallenge(
  challenge: Challenge,
  chain: Chain,
  address: string,
): void {
  if (challenge.chain !== chain)
    throw new Error("The sign-in challenge is for a different network.");
  requireActor(address, challenge.address, chain);
  if (Date.parse(challenge.expires_at) <= Date.now())
    throw new Error("This sign-in request has expired. Please start again.");
  if (challenge.recipient !== location.host)
    throw new Error("The sign-in request belongs to a different website.");
}

export function networkIdentity(chain: Chain, config: PublicConfig): string {
  if (chain === "ethereum") return config.ethereum.genesis_hash;
  if (chain === "solana") return config.solana.genesis_hash;
  return JSON.stringify(config.near.genesis);
}

const journalKey = "apodect.wallet-transactions";
const activeSubmissions = new Set<string>();

export function walletTransactions(): WalletTransaction[] {
  try {
    return JSON.parse(
      localStorage.getItem(journalKey) ?? "[]",
    ) as WalletTransaction[];
  } catch {
    return [];
  }
}

function recordTransaction(transaction: WalletTransaction): void {
  const records = walletTransactions().filter(
    (item) => item.id !== transaction.id,
  );
  records.push(transaction);
  localStorage.setItem(journalKey, JSON.stringify(records));
}

/** A timeout is unresolved execution. Never submit the next financial step after it. */
export async function runTransactions(input: {
  chain: Chain;
  actor: string;
  config: PublicConfig;
  total: number;
  intent: UnsignedIntent;
  submission: "wallet" | "signed";
  progress: (event: WalletProgress) => void;
  submit: (
    index: number,
    record: (transactionId: string) => void,
    beginSubmission: (nonces: Record<string, string>) => void,
  ) => Promise<string>;
}): Promise<WalletTransaction[]> {
  const previous = walletTransactions();
  const network = networkIdentity(input.chain, input.config);
  if (
    previous.some(
      (transaction) =>
        transaction.chain === input.chain &&
        transaction.network === network &&
        sameAddress(transaction.actor, input.actor, input.chain) &&
        ["submitted", "unknown"].includes(transaction.status),
    )
  ) {
    throw new WalletExecutionError(
      "This wallet has an unresolved transaction. Check its confirmation in Account & wallets before submitting another action.",
      previous.filter(
        (transaction) =>
          transaction.chain === input.chain && transaction.network === network,
      ),
    );
  }
  // Verify persistence before opening the wallet. Only public transaction
  // identities and outcomes are retained; private keys never enter this journal.
  try {
    localStorage.setItem(journalKey, JSON.stringify(previous));
  } catch {
    throw new Error(
      "Enable browser storage so submitted transactions can be recovered after a reload.",
    );
  }
  const transactions: WalletTransaction[] = [];
  for (let index = 0; index < input.total; index++) {
    const expected =
      input.intent.chain === "solana"
        ? input.intent
        : ({
            ...input.intent,
            transactions: [input.intent.transactions[index]],
          } as UnsignedIntent);
    const submitted: WalletTransaction = {
      id: crypto.randomUUID(),
      chain: input.chain,
      actor: input.actor,
      network,
      transactionId: null,
      status: "unknown",
      expected,
      submission: input.submission,
    };
    transactions.push(submitted);
    recordTransaction(submitted);
    activeSubmissions.add(submitted.id);
    const emit = (stage: WalletProgress["stage"], message: string) =>
      input.progress({
        stage,
        message,
        chain: input.chain,
        step: index + 1,
        total: input.total,
        transactionId: submitted.transactionId ?? undefined,
      });
    const record = (transactionId: string) => {
      if (!submitted.transactionId) {
        submitted.transactionId = transactionId;
        submitted.status = "submitted";
        recordTransaction(submitted);
        emit("submitted", "Transaction submitted. Waiting for the network.");
      }
    };
    const beginSubmission = (nonces: Record<string, string>) => {
      submitted.nonceContext = nonces;
      recordTransaction(submitted);
    };

    try {
      emit("approval", "Review and approve this transaction in your wallet.");
      record(await input.submit(index, record, beginSubmission));
      emit("confirming", "Waiting for final confirmation.");
      await confirmTransaction(submitted);
      submitted.status = "finalized";
      recordTransaction(submitted);
      emit(
        "finalized",
        "Transaction finalized. Project balances will update after indexing.",
      );
    } catch (error) {
      const rejected =
        error instanceof TransactionNotSubmitted ||
        (typeof error === "object" &&
          error !== null &&
          "code" in error &&
          error.code === 4001);
      const failed =
        error instanceof TransactionFailed ||
        (!submitted.transactionId &&
          (input.submission === "signed" ||
            !submitted.nonceContext ||
            rejected));
      submitted.status = failed ? "failed" : "unknown";
      recordTransaction(submitted);
      const message =
        error instanceof Error
          ? error.message
          : "The wallet could not complete this action.";
      emit(failed ? "failed" : "unknown", message);
      throw new WalletExecutionError(message, transactions);
    } finally {
      activeSubmissions.delete(submitted.id);
    }
  }
  return transactions;
}

export function discardUnsubmittedTransaction(
  transaction: WalletTransaction,
): void {
  if (activeSubmissions.has(transaction.id))
    throw new Error(
      "The wallet is still handling this request. Close or reject it there first.",
    );
  if (
    (transaction.submission !== "signed" && transaction.nonceContext) ||
    transaction.transactionId
  )
    throw new Error(
      "This transaction may have been sent by the wallet. Check its history instead.",
    );
  transaction.status = "failed";
  recordTransaction(transaction);
}

/** Recover a wallet broadcast whose response was lost, without authorizing a replacement. */
export async function resolveWalletTransaction(
  transaction: WalletTransaction,
  hash: string,
  config: PublicConfig,
): Promise<WalletTransaction> {
  if (!hash.trim())
    throw new Error("Enter the transaction hash from your wallet history.");
  if (transaction.network !== networkIdentity(transaction.chain, config))
    throw new Error("This transaction belongs to another deployment.");
  if (transaction.expected.chain === "ethereum") {
    await (
      await import("./wallets/ethereum")
    ).verifyTransaction(
      hash.trim(),
      transaction.expected,
      transaction.nonceContext,
    );
  } else if (transaction.expected.chain === "near") {
    await (
      await import("./wallets/near")
    ).verifyTransaction(
      hash.trim(),
      transaction.expected,
      transaction.nonceContext,
    );
  } else {
    throw new Error(
      "Solana transactions are recorded before broadcasting. Check the recorded hash.",
    );
  }
  transaction.transactionId = hash.trim();
  transaction.status = "submitted";
  recordTransaction(transaction);
  return refreshWalletTransaction(transaction, config);
}

export async function refreshWalletTransaction(
  transaction: WalletTransaction,
  config: PublicConfig,
): Promise<WalletTransaction> {
  if (transaction.network !== networkIdentity(transaction.chain, config))
    throw new Error("This transaction belongs to another deployment.");
  try {
    await confirmTransaction(transaction);
    transaction.status = "finalized";
  } catch (error) {
    transaction.status =
      error instanceof TransactionFailed ? "failed" : "unknown";
    recordTransaction(transaction);
    throw error;
  }
  recordTransaction(transaction);
  return transaction;
}

async function confirmTransaction(
  transaction: WalletTransaction,
): Promise<void> {
  if (!transaction.transactionId)
    throw new Error(
      "The wallet did not return a hash. Find the transaction in its history to check the outcome.",
    );
  const started = Date.now();
  while (Date.now() - started < 120_000) {
    const done =
      transaction.chain === "ethereum"
        ? await (
            await import("./wallets/ethereum")
          ).isFinalized(transaction.transactionId)
        : transaction.chain === "solana"
          ? await (
              await import("./wallets/solana")
            ).isFinalized(transaction.transactionId)
          : await (
              await import("./wallets/near")
            ).isFinalized(transaction.transactionId, transaction.actor);
    if (done) return;
    await new Promise((resolve) => setTimeout(resolve, 1_500));
  }
  throw new Error(
    "Confirmation is still pending. The transaction may have executed; check its status before trying again.",
  );
}
