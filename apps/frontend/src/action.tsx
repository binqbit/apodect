import { useEffect, useRef, useState, type FormEvent } from "react";
import { Link, useLocation } from "react-router-dom";
import {
  api,
  type ApplyPreparation,
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
  currentActionTarget,
  actionAmountLimit,
  slippageBasisPoints,
  type ActionTarget,
  type ActionFieldsValue,
  type ChangeField,
} from "./action-model";
export type { ActionTarget } from "./action-model";

export function ActionDialog({
  project,
  target: selectedTarget,
  onClose,
  onSubmitted,
}: {
  project: ProjectDetail;
  target: ActionTarget;
  onClose: () => void;
  onSubmitted: () => void;
}) {
  const { account, config, wallets, rememberWallet } = useSession();
  const location = useLocation();
  const returnTo = location.pathname + location.search + location.hash;
  const target = currentActionTarget(project, selectedTarget);
  const { kind } = target;
  const local = localKinds.includes(kind);
  const invoiceAmount = target.invoice
    ? kind === "fund" && target.invoice.intended !== null
      ? (BigInt(target.invoice.intended) > BigInt(target.invoice.actual_funding)
          ? BigInt(target.invoice.intended) -
            BigInt(target.invoice.actual_funding)
          : 0n
        ).toString()
      : target.invoice.amount
    : null;
  const [values, setValues] = useState<ActionFieldsValue>({
    chain:
      target.chain ??
      target.task?.chain ??
      target.request?.chain ??
      account?.session.wallet.chain ??
      "ethereum",
    actor: "",
    amount:
      invoiceAmount && BigInt(invoiceAmount) > 0n
        ? money(invoiceAmount).replaceAll(",", "")
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
  const change: ChangeField = (field, value) => {
    setValues((previous) => ({ ...previous, [field]: value }));
    setPrepared(null);
  };
  const [tokenAccounts, setTokenAccounts] = useState<
    { address: string; amount: string }[]
  >([]);
  const [tokenError, setTokenError] = useState("");
  const [tokenLoading, setTokenLoading] = useState(false);
  const [tokenRevision, setTokenRevision] = useState(0);
  const [prepared, setPrepared] = useState<{
    intent: UnsignedIntent;
    costs: FundingCosts | null;
    expires: number | null;
    key: string;
    values: ActionFieldsValue;
    target: ActionTarget;
    project: ProjectDetail;
  } | null>(null);
  const intent = prepared?.intent ?? null;
  const [waiting, setWaiting] = useState<Extract<
    ApplyPreparation,
    { status: "waiting" }
  > | null>(null);
  const [error, setError] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<WalletProgress[]>([]);
  const [receipts, setReceipts] = useState<WalletTransaction[] | null>(null);
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
      if (
        wallet.chain !== signingChain ||
        wallet.network !== config?.wallet_networks[signingChain]
      )
        return false;
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
  const eligibleAddresses = JSON.stringify(
    eligibleWallets.map((wallet) => wallet.address),
  );
  useEffect(() => {
    setValues((previous) => {
      const eligible = eligibleWallets.find((wallet) =>
        sameAddress(wallet.address, previous.actor, signingChain),
      );
      const actor = eligible?.address ?? eligibleWallets[0]?.address ?? "";
      return actor === previous.actor
        ? previous
        : { ...previous, actor, tokenAccount: "" };
    });
  }, [signingChain, eligibleAddresses]);
  const amountLimit = actionAmountLimit(project, target, chain);
  const taskId = target.task?.id ?? target.request?.task;
  const taskChain = target.task?.chain ?? target.request?.chain;
  const preparationKey = JSON.stringify({
    project: project.key,
    profile: account?.session.profile.id,
    owner: project.owner,
    authority: project.authority,
    retryLimit: project.retry_limit,
    eligibleAddresses,
    target,
    taskCommands: project.commands
      .filter(
        (command) => command.chain === taskChain && command.task === taskId,
      )
      .map(({ id, action, status }) => ({ id, action, status })),
    amountLimit,
    values,
  });
  const latestPreparationKey = useRef(preparationKey);
  latestPreparationKey.current = preparationKey;
  useEffect(() => {
    if (!busy && !receipts && prepared && prepared.key !== preparationKey) {
      setPrepared(null);
      setError(
        new Error(
          "The project state or signing permissions changed. Review a fresh transaction before signing.",
        ),
      );
    }
  }, [busy, receipts, prepared, preparationKey]);
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
    setTokenLoading(true);
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
          const accounts = result.value
            .map((item) => ({
              address: item.pubkey,
              amount: item.account.data.parsed.info.tokenAmount.amount,
            }))
            .sort((left, right) =>
              BigInt(left.amount) > BigInt(right.amount)
                ? -1
                : BigInt(left.amount) < BigInt(right.amount)
                  ? 1
                  : 0,
            );
          setTokenAccounts(accounts);
          change("tokenAccount", accounts[0]?.address ?? "");
          if (!accounts.length && kind !== "withdraw")
            setTokenError(
              "This wallet has no USDC token account on this network. Fund this wallet with USDC before continuing.",
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
      )
      .finally(() => {
        if (current) setTokenLoading(false);
      });
    return () => {
      current = false;
    };
  }, [needsToken, actor, project.key, tokenRevision]);

  async function prepare(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(undefined);
    setWaiting(null);
    try {
      if (
        ["task", "request", "plan", "invoice", "command"].some(
          (field) =>
            selectedTarget[field as keyof ActionTarget] &&
            !target[field as keyof ActionTarget],
        )
      )
        throw new Error(
          "The selected record is no longer available. Close this action and refresh the project.",
        );
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
      if (amountLimit !== null && BigInt(parsedAmount) > BigInt(amountLimit))
        throw new Error(
          `The amount exceeds the confirmed limit of ${money(amountLimit)} USDC. Refresh the project if funds have changed.`,
        );
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
        if (new TextEncoder().encode(value.trim()).length > 16_000)
          throw new Error(
            "Shorten the explanation to at most 16,000 UTF-8 bytes.",
          );
        return (await api.saveReference(project.key, value.trim())).digest;
      }
      const purposeRef = purposeKinds.includes(kind)
        ? await saveReference(purpose)
        : null;
      const reportRef = ["finalize", "respond_invoice"].includes(kind)
        ? await saveReference(report, true)
        : null;
      let prepared: UnsignedIntent;
      let costs: FundingCosts | null = null;
      if (kind === "apply") {
        const bps = slippageBasisPoints(slippage);
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
          setWaiting(reply);
          return;
        }
        prepared = reply.intent;
        costs = reply.costs;
      } else if (local) {
        if (needsToken && (tokenLoading || tokenError))
          throw new Error(
            "Wait for a successful USDC token account lookup before preparing this action.",
          );
        if (needsToken && kind !== "withdraw" && !tokenAccount)
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
          ...(needsToken && tokenAccount
            ? { token_account: tokenAccount }
            : {}),
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
      if (latestPreparationKey.current !== preparationKey)
        throw new Error(
          "The project state or signing permissions changed while preparing. Review a fresh transaction.",
        );
      setPrepared({
        intent: prepared,
        costs,
        expires: expiringKinds.includes(kind) ? expires : null,
        key: preparationKey,
        values: { ...values },
        target,
        project,
      });
    } catch (failure) {
      setError(failure);
    } finally {
      setBusy(false);
    }
  }
  async function sign(wallet: ConnectedWallet) {
    if (!prepared || busy || receipts) return;
    const { intent, expires } = prepared;
    setBusy(true);
    setError(undefined);
    try {
      requireActor(wallet.address, intent.actor, intent.chain);
      if (prepared.key !== latestPreparationKey.current)
        throw new Error(
          "The project state changed. Go back and prepare a fresh review.",
        );
      if (expires !== null && expires <= Math.floor(Date.now() / 1000))
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
  const unresolved =
    receipts?.some((receipt) =>
      ["submitted", "unknown"].includes(receipt.status),
    ) ?? false;
  const allFinalized =
    receipts?.every((receipt) => receipt.status === "finalized") ?? false;
  const nothingSubmitted =
    receipts?.every(
      (receipt) =>
        receipt.status === "failed" && receipt.transactionId === null,
    ) ?? false;
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
        {waiting !== null && (
          <Notice>
            <strong>
              {waiting.reason === "provider_quote_unavailable"
                ? "A funding route is currently unavailable."
                : "Waiting for project funding."}
            </strong>
            <p>
              {waiting.reason === "provider_quote_unavailable"
                ? "The provider could not quote all of the remaining amount. Try again or fund the destination network. Remaining: "
                : "The full request cannot currently be covered. Shortfall: "}
              <Money value={waiting.shortfall} />. No allocation was made.
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
              tokenLoading={tokenLoading}
              tokenAccounts={tokenAccounts}
              onTokenRetry={() => setTokenRevision((revision) => revision + 1)}
              amountLimit={amountLimit}
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
              <button
                className="button"
                disabled={
                  busy ||
                  !actor ||
                  (needsToken && (tokenLoading || !!tokenError))
                }
              >
                {busy ? "Preparing…" : "Review transaction →"}
              </button>
            </div>
          </form>
        ) : (
          <div className="form-stack">
            <ActionReview
              project={prepared!.project}
              target={prepared!.target}
              values={prepared!.values}
              intent={intent}
              financialAmount={financialAmount}
              costs={prepared!.costs}
              preparedExpires={prepared!.expires}
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
                <Notice tone={allFinalized ? "success" : "info"}>
                  {allFinalized
                    ? "Wallet transactions finalized. The project view updates after the indexer confirms the resulting state. Cross-chain delivery, when needed, continues separately."
                    : unresolved
                      ? "The wallet flow is incomplete. Check its recorded transactions in Account & wallets before preparing another payment. An unknown result never means the transfer failed."
                      : nothingSubmitted
                        ? "Nothing was submitted. You can review the action again when your wallet is ready."
                        : "The failed step has a confirmed outcome. Earlier successful steps remain recorded; prepare a fresh action to continue from current chain state."}
                </Notice>
                <div className="button-row">
                  <button className="button" onClick={close}>
                    Back to project
                  </button>
                  {!allFinalized && !unresolved && (
                    <button
                      className="button secondary"
                      onClick={() => {
                        setPrepared(null);
                        setReceipts(null);
                        setError(undefined);
                        setProgress([]);
                      }}
                    >
                      Review again
                    </button>
                  )}
                  {unresolved && (
                    <Link
                      to="/app/account"
                      state={{ returnTo }}
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
                      setPrepared(null);
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
                      state={{ returnTo, chain: signingChain }}
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
