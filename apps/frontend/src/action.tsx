import { useEffect, useRef, useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import {
  api,
  type FundingCosts,
  type ProjectDetail,
  type UnsignedIntent,
} from "./api";
import { useSession } from "./app";
import { WalletPicker } from "./pages";
import {
  WalletExecutionError,
  requireActor,
  sameAddress,
  type ConnectedWallet,
  type WalletProgress,
  type WalletTransaction,
} from "./wallets";
import { Money, Notice, baseUnits, money } from "./ui";
import { ActionFields, ActionReview } from "./action-view";
import {
  copy,
  localKinds,
  amountKinds,
  purposeKinds,
  expiringKinds,
  ownerKinds,
  reference,
  nativeAmount,
  buildLocalAction,
  buildCoordinatorAction,
  type ActionTarget,
  type ActionFieldsValue,
  type ChangeField,
} from "./action-model";
export type { ActionTarget } from "./action-model";

export function ActionDialog({
  project,
  target,
  onClose,
  onSubmitted,
}: {
  project: ProjectDetail;
  target: ActionTarget;
  onClose: () => void;
  onSubmitted: () => void;
}) {
  const { account, wallets, rememberWallet } = useSession();
  const { kind } = target;
  const local = localKinds.includes(kind);
  const [values, setValues] = useState<ActionFieldsValue>({
    chain:
      target.chain ??
      target.task?.chain ??
      target.request?.chain ??
      account?.session.wallet.chain ??
      "ethereum",
    actor: "",
    amount: target.invoice
      ? money(target.invoice.amount).replaceAll(",", "")
      : "",
    purpose: "",
    report: "",
    existingReference: false,
    manager: "",
    attempts: String(project.retry_limit),
    slippage: "0.5",
    minutes: "20",
    storage: "0.05",
    reason: "",
    unverified: "",
    tokenAccount: "",
  });
  const {
    chain,
    actor,
    amount,
    purpose,
    report,
    existingReference,
    slippage,
    minutes,
    storage,
    tokenAccount,
  } = values;
  const signingChain = local ? chain : "near";
  const change: ChangeField = (field, value) =>
    setValues((previous) => ({ ...previous, [field]: value }));
  const [tokenAccounts, setTokenAccounts] = useState<
    { address: string; amount: string }[]
  >([]);
  const [tokenError, setTokenError] = useState("");
  const [intent, setIntent] = useState<UnsignedIntent | null>(null);
  const [costs, setCosts] = useState<FundingCosts | null>(null);
  const [shortfall, setShortfall] = useState<string | null>(null);
  const [error, setError] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<WalletProgress[]>([]);
  const [receipts, setReceipts] = useState<WalletTransaction[] | null>(null);
  const [preparedExpires, setPreparedExpires] = useState<number | null>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const needsToken =
    local &&
    chain === "solana" &&
    ["fund", "return_funds", "withdraw"].includes(kind);
  const tokenIngress = ["fund", "return_funds"].includes(kind);
  const needsStorage =
    signingChain === "near" && kind !== "apply" && !(local && tokenIngress);
  const eligibleWallets =
    account?.wallets.filter((wallet) => {
      if (wallet.chain !== signingChain) return false;
      const expected = local
        ? (target.task?.agent ??
          (kind === "fund" ? project.roles.local_owners[chain] : null))
        : ownerKinds.includes(kind)
          ? project.roles.owner_actor
          : ["abort", "invalidate"].includes(kind)
            ? null
            : project.roles.authority_actor;
      if (!expected && !local)
        return [project.owner, project.authority].includes(wallet.address);
      return !expected || sameAddress(wallet.address, expected, signingChain);
    }) ?? [];
  useEffect(() => {
    change("actor", eligibleWallets[0]?.address ?? "");
    setIntent(null);
    setCosts(null);
  }, [signingChain, chain]);
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    dialog.current?.focus();
    return () => {
      document.body.style.overflow = previousOverflow;
      before?.focus();
    };
  }, []);
  useEffect(() => {
    if (!needsToken || !actor) return;
    let current = true;
    const asset = project.chains.find((view) => view.chain === "solana")!.asset;
    setTokenAccounts([]);
    change("tokenAccount", "");
    setTokenError("");
    api
      .rpc<{
        value: {
          pubkey: string;
          account: {
            data: { parsed: { info: { tokenAmount: { amount: string } } } };
          };
        }[];
      }>("solana", "getTokenAccountsByOwner", [
        actor,
        { mint: asset },
        { encoding: "jsonParsed", commitment: "finalized" },
      ])
      .then(
        (result) => {
          if (!current) return;
          const accounts = result.value.map((item) => ({
            address: item.pubkey,
            amount: item.account.data.parsed.info.tokenAmount.amount,
          }));
          setTokenAccounts(accounts);
          change("tokenAccount", accounts[0]?.address ?? "");
          if (!accounts.length)
            setTokenError(
              "This wallet has no USDC token account on this network. Create one in your wallet before continuing.",
            );
        },
        (failure: unknown) => {
          if (current)
            setTokenError(
              failure instanceof Error
                ? failure.message
                : "Unable to read token accounts.",
            );
        },
      );
    return () => {
      current = false;
    };
  }, [needsToken, actor, project.key]);

  async function prepare(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(undefined);
    setShortfall(null);
    try {
      if (!actor)
        throw new Error(
          "Link the wallet with the required project role before preparing this action.",
        );
      const parsedAmount = amountKinds.includes(kind) ? baseUnits(amount) : "0";
      if (
        amountKinds.includes(kind) &&
        kind !== "respond_invoice" &&
        BigInt(parsedAmount) === 0n
      )
        throw new Error("The amount must be greater than zero.");
      const expires = Math.floor(Date.now() / 1000) + Number(minutes) * 60;
      if (
        !Number.isSafeInteger(expires) ||
        Number(minutes) < 1 ||
        Number(minutes) > 1440
      )
        throw new Error("Choose a validity between 1 minute and 24 hours.");
      async function saveReference(
        value: string,
        optional = false,
      ): Promise<string | null> {
        if (existingReference) return reference(value, optional);
        if (!value.trim()) {
          if (optional) return null;
          throw new Error("Describe the purpose of this request.");
        }
        return (await api.saveReference(project.key, value.trim())).digest;
      }
      const purposeRef = purposeKinds.includes(kind)
        ? await saveReference(purpose)
        : null;
      const reportRef = ["finalize", "respond_invoice"].includes(kind)
        ? await saveReference(report, true)
        : null;
      let prepared: UnsignedIntent;
      if (kind === "apply") {
        const slippageUnits = BigInt(baseUnits(slippage));
        const bps = Number(slippageUnits / 10_000n);
        if (slippageUnits % 10_000n !== 0n || bps > 10_000)
          throw new Error(
            "Slippage must be between 0 and 100%, with at most two decimals.",
          );
        const reply = await api.prepareApply(project.key, {
          actor,
          request: {
            destination: chain,
            request: target.request!.id,
            slippage_bps: bps,
            expires,
          },
        });
        if (reply.status === "waiting") {
          setShortfall(reply.shortfall);
          return;
        }
        prepared = reply.intent;
        setCosts(reply.costs);
      } else if (local) {
        if (needsToken && !tokenAccount)
          throw new Error("Choose a USDC token account owned by this wallet.");
        prepared = await api.prepareLocal(project.key, {
          actor,
          chain,
          action: buildLocalAction(
            target,
            values,
            parsedAmount,
            purposeRef,
            reportRef,
          ),
          ...(chain === "near"
            ? { storage_deposit: tokenIngress ? "1" : nativeAmount(storage) }
            : {}),
          ...(needsToken ? { token_account: tokenAccount } : {}),
        });
      } else {
        prepared = await api.prepareCoordinator(project.key, {
          actor,
          action: buildCoordinatorAction(
            target,
            values,
            parsedAmount,
            expires,
            purposeRef,
            reportRef,
          ),
          storage_deposit: nativeAmount(storage),
        });
      }
      setPreparedExpires(expiringKinds.includes(kind) ? expires : null);
      setIntent(prepared);
    } catch (failure) {
      setError(failure);
    } finally {
      setBusy(false);
    }
  }
  async function sign(wallet: ConnectedWallet) {
    if (!intent || busy || receipts) return;
    setBusy(true);
    setError(undefined);
    try {
      requireActor(wallet.address, intent.actor, intent.chain);
      if (
        preparedExpires !== null &&
        preparedExpires <= Math.floor(Date.now() / 1000)
      )
        throw new Error(
          "This action has expired. Go back and prepare a fresh review.",
        );
      rememberWallet(wallet);
      const result = await wallet.execute(intent, (update) =>
        setProgress((previous) => [
          ...previous.filter((item) => item.step !== update.step),
          update,
        ]),
      );
      setReceipts(result);
      onSubmitted();
    } catch (failure) {
      setError(failure);
      if (
        failure instanceof WalletExecutionError &&
        failure.transactions.length > 0
      ) {
        setReceipts(failure.transactions);
        onSubmitted();
      }
    } finally {
      setBusy(false);
    }
  }
  const wallet = wallets.find(
    (item) =>
      item.chain === signingChain &&
      sameAddress(item.address, actor, signingChain),
  );
  function close() {
    if (!busy) onClose();
  }
  const financialAmount =
    target.request?.amount ??
    (amount && amountKinds.includes(kind)
      ? (() => {
          try {
            return baseUnits(amount);
          } catch {
            return null;
          }
        })()
      : null);
  return (
    <div
      className="action-tray"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    >
      <div
        className="action-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="action-title"
        ref={dialog}
        tabIndex={-1}
        onKeyDown={(event) => {
          if (event.key === "Escape") close();
          if (event.key === "Tab") {
            const elements = dialog.current?.querySelectorAll<HTMLElement>(
              'button:not(:disabled), a[href], input:not(:disabled), textarea:not(:disabled), select:not(:disabled), summary, [tabindex="0"]',
            );
            if (!elements?.length) return;
            const first = elements[0];
            const last = elements[elements.length - 1];
            if (
              event.shiftKey &&
              (document.activeElement === first ||
                document.activeElement === dialog.current)
            ) {
              event.preventDefault();
              last.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first.focus();
            }
          }
        }}
      >
        <div className="dialog-heading">
          <div>
            <h2 id="action-title">{copy[kind].title}</h2>
            <p>{copy[kind].description}</p>
          </div>
          <button
            className="close-button"
            aria-label="Close action"
            disabled={busy}
            onClick={close}
          >
            ×
          </button>
        </div>
        {error !== undefined && (
          <Notice tone="error">
            {error instanceof Error
              ? error.message
              : "This action could not be completed."}
          </Notice>
        )}
        {shortfall !== null && (
          <Notice>
            <strong>Waiting for project funding.</strong>
            <p>
              The full request cannot currently be covered. Shortfall:{" "}
              <Money value={shortfall} />. No allocation was made.
            </p>
          </Notice>
        )}
        {!intent ? (
          <form
            onSubmit={(event) => void prepare(event)}
            className="form-stack"
          >
            <ActionFields
              values={values}
              change={change}
              target={target}
              busy={busy}
              eligibleWallets={eligibleWallets}
              needsToken={needsToken}
              tokenError={tokenError}
              tokenAccounts={tokenAccounts}
              needsStorage={needsStorage}
              signingChain={signingChain}
              onClose={close}
            />
            <div className="form-actions">
              <button
                type="button"
                className="button secondary"
                onClick={close}
                disabled={busy}
              >
                Cancel
              </button>
              <button className="button" disabled={busy || !actor}>
                {busy ? "Preparing…" : "Review transaction →"}
              </button>
            </div>
          </form>
        ) : (
          <div className="form-stack">
            <ActionReview
              kind={kind}
              intent={intent}
              financialAmount={financialAmount}
              costs={costs}
              preparedExpires={preparedExpires}
            />
            {progress.length > 0 && (
              <ul className="progress-list" aria-live="polite">
                {progress.map((item) => (
                  <li key={item.step}>
                    <span>
                      {item.stage === "finalized"
                        ? "✓"
                        : item.stage === "failed"
                          ? "!"
                          : "·"}
                    </span>
                    <div>
                      Step {item.step} of {item.total}: {item.message}
                      {item.transactionId && (
                        <div>
                          <code>{item.transactionId}</code>
                        </div>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            )}
            {receipts ? (
              <>
                <Notice
                  tone={
                    receipts.every((item) => item.status === "finalized")
                      ? "success"
                      : "info"
                  }
                >
                  {receipts.every((item) => item.status === "finalized")
                    ? "Wallet transactions finalized. The project view updates after the indexer confirms the resulting state. Cross-chain delivery, when needed, continues separately."
                    : "The wallet flow is incomplete. Check its recorded transactions in Account & wallets before preparing another payment. An unknown result never means the transfer failed."}
                </Notice>
                <div className="button-row">
                  <button className="button" onClick={close}>
                    Back to project
                  </button>
                  {!receipts.every((item) => item.status === "finalized") && (
                    <Link
                      to="/app/account"
                      onClick={close}
                      className="button secondary"
                    >
                      Check wallet transactions
                    </Link>
                  )}
                </div>
              </>
            ) : (
              <>
                <div className="form-actions">
                  <button
                    className="button secondary"
                    disabled={busy}
                    onClick={() => {
                      setIntent(null);
                      setCosts(null);
                      setError(undefined);
                      setProgress([]);
                    }}
                  >
                    Edit details
                  </button>
                  {wallet && (
                    <button
                      className="button"
                      disabled={busy}
                      onClick={() => void sign(wallet)}
                    >
                      {busy ? "Waiting for wallet…" : "Sign with wallet"}
                    </button>
                  )}
                </div>
                {!wallet && (
                  <div>
                    <p className="review-note" style={{ marginBottom: 12 }}>
                      Reconnect the signing wallet to approve these
                      transactions. Select account {intent.actor}.
                    </p>
                    <WalletPicker
                      chain={intent.chain}
                      disabled={busy}
                      onConnect={sign}
                    />
                    <Link
                      className="text-link"
                      to="/app/account"
                      onClick={close}
                    >
                      Manage connections in Account & wallets →
                    </Link>
                  </div>
                )}
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
