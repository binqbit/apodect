import { useEffect, useRef, useState, type FormEvent } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import {
  api,
  type Account,
  type Chain,
  type ProjectRoles,
  type PublicConfig,
} from "./api";
import { useSession } from "./app";
import {
  discoverWallets,
  importDevelopmentWallet,
  refreshWalletTransaction,
  resolveWalletTransaction,
  discardUnsubmittedTransaction,
  walletTransactions,
  sameAddress,
  networkIdentity,
  type WalletTransaction,
  type ConnectedWallet,
  type WalletOption,
} from "./wallets";
import {
  Address,
  ChainBadge,
  Empty,
  ErrorState,
  Field,
  GuideLink,
  Loading,
  Money,
  Notice,
  PageTitle,
  Panel,
  Status,
  chainNames,
  chains,
  short,
  useLoad,
} from "./ui";

export function RoleBadges({ roles }: { roles: ProjectRoles }) {
  const ownedChains = chains.filter((chain) => roles.local_owners[chain]);
  return (
    <div className="role-badges">
      {roles.owner && <span className="role-badge">Owner</span>}
      {roles.authority && <span className="role-badge">Manager</span>}
      {roles.agent && <span className="role-badge">Agent</span>}
      {!roles.owner &&
        ownedChains.map((chain) => (
          <span className="role-badge" key={chain}>
            Vault owner · {chainNames[chain]}
          </span>
        ))}
      {!roles.owner &&
        !roles.authority &&
        !roles.agent &&
        ownedChains.length === 0 && (
          <span className="role-badge">Participant</span>
        )}
    </div>
  );
}

export function WalletPicker({
  chain,
  onConnect,
  onBusyChange,
  disabled = false,
}: {
  chain: Chain;
  onConnect: (wallet: ConnectedWallet) => void | Promise<void>;
  onBusyChange?: (busy: boolean) => void;
  disabled?: boolean;
}) {
  const { config, rememberWallet } = useSession();
  const [options, setOptions] = useState<WalletOption[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    let current = true;
    setOptions(null);
    setError(undefined);
    if (config)
      discoverWallets(chain, config).then(
        (value) => current && setOptions(value),
        (failure: unknown) => current && setError(failure),
      );
    return () => {
      current = false;
    };
  }, [chain, config]);
  async function connect(option: WalletOption) {
    if (!config) return;
    setBusy(true);
    onBusyChange?.(true);
    setError(undefined);
    try {
      const wallet = await option.connect();
      if (!mounted.current) return;
      rememberWallet(wallet);
      await onConnect(wallet);
    } catch (failure) {
      if (mounted.current) setError(failure);
    } finally {
      if (mounted.current) {
        setBusy(false);
        onBusyChange?.(false);
      }
    }
  }
  return (
    <div>
      {error !== undefined && (
        <Notice tone="error">
          {error instanceof Error
            ? error.message
            : "The wallet could not connect."}
        </Notice>
      )}
      {options === null && !error && <Loading>Looking for wallets…</Loading>}
      {options?.length === 0 && (
        <Notice>
          No compatible {chain === "near" ? "NEAR" : chain} wallet was found.
          Open this page in your wallet browser or enable a compatible wallet
          extension, then reload.
        </Notice>
      )}
      <div className="button-row wallet-providers">
        {options?.map((option) => (
          <button
            type="button"
            key={option.id}
            className="button secondary"
            disabled={busy || disabled}
            onClick={() => void connect(option)}
          >
            {busy ? "Waiting for wallet…" : option.name}
          </button>
        ))}
      </div>
    </div>
  );
}

export function AccountPage() {
  const route = useLocation();
  const navigate = useNavigate();
  const requestedPath: unknown = route.state?.returnTo;
  const returnTo =
    typeof requestedPath === "string" &&
    (requestedPath === "/app" || requestedPath.startsWith("/app/projects/"))
      ? requestedPath
      : "/app";
  const {
    account,
    config,
    loading,
    error: sessionError,
    setAccount,
    refreshAccount,
    rememberWallet,
  } = useSession();
  const [chain, setChain] = useState<Chain>(() =>
    chains.includes(route.state?.chain) ? route.state.chain : "ethereum",
  );
  const [name, setName] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  useEffect(
    () => setName(account?.session.profile.display_name ?? ""),
    [account?.session.profile.display_name],
  );
  async function authenticate(wallet: ConnectedWallet) {
    setBusy(true);
    setMessage("");
    setError(undefined);
    try {
      const challenge = account
        ? await api.linkChallenge(
            wallet.chain,
            wallet.address,
            wallet.callbackUrl,
          )
        : await api.challenge(wallet.chain, wallet.address, wallet.callbackUrl);
      const signature = await wallet.signChallenge(challenge);
      const proof = { ...signature, challenge: challenge.id };
      setAccount(
        account ? await api.linkWallet(proof) : await api.login(proof),
      );
      setMessage(
        account
          ? "Wallet ownership verified. The wallet is now linked to this profile."
          : "Wallet verified. Your workspace is ready.",
      );
      if (!account && requestedPath) navigate(returnTo, { replace: true });
    } finally {
      setBusy(false);
    }
  }
  async function importLocal(file: File) {
    if (!config) return;
    setBusy(true);
    setError(undefined);
    try {
      const wallet = await importDevelopmentWallet(file, config);
      rememberWallet(wallet);
      await authenticate(wallet);
    } catch (failure) {
      setError(failure);
    } finally {
      setBusy(false);
    }
  }
  async function rename(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(undefined);
    setMessage("");
    try {
      await api.rename(name.trim() || null);
      await refreshAccount();
      setMessage("Profile updated.");
    } catch (failure) {
      setError(failure);
    } finally {
      setBusy(false);
    }
  }
  async function logout() {
    setBusy(true);
    setError(undefined);
    try {
      await api.logout();
      setAccount(null);
      setMessage("");
    } catch (failure) {
      setError(failure);
    } finally {
      setBusy(false);
    }
  }
  if (loading) return <Loading>Loading wallet configuration…</Loading>;
  if (sessionError)
    return <ErrorState error={sessionError} retry={() => location.reload()} />;
  return (
    <>
      <PageTitle
        eyebrow={account ? "Your identity" : "Welcome to Apodect"}
        title={account ? "Account & wallets" : "Sign in with your wallet"}
        action={
          account && (
            <button
              className="button secondary small"
              disabled={busy}
              onClick={() => void logout()}
            >
              Sign out
            </button>
          )
        }
      >
        {account
          ? "Manage the wallets you use across projects and networks."
          : "Sign a message to create your profile or return with a linked wallet. No transaction or token approval is needed to sign in."}
      </PageTitle>
      <p className="page-help">
        <GuideLink section="wallets">Sign-in and wallet guide →</GuideLink>
      </p>
      {!account && requestedPath && (
        <Notice>Sign in to return to the page you opened.</Notice>
      )}
      {message && (
        <Notice tone="success">
          {message}
          {account && (
            <p>
              <Link className="text-link" to={returnTo}>
                {returnTo === "/app"
                  ? "Go to projects →"
                  : "Return to project →"}
              </Link>
            </p>
          )}
        </Notice>
      )}
      {error !== undefined && (
        <Notice tone="error">
          {error instanceof Error
            ? error.message
            : "The account could not be updated."}
        </Notice>
      )}
      <div className="account-layout">
        <div>
          {account && (
            <Panel
              title={account.session.profile.display_name ?? "Your profile"}
            >
              <details className="disclosure panel-disclosure">
                <summary>Edit display name</summary>
                <form
                  className="panel-body form-stack"
                  onSubmit={(event) => void rename(event)}
                >
                  <Field
                    label="Display name"
                    hint="Optional. This is how your name appears in the workspace."
                  >
                    <input
                      value={name}
                      onChange={(event) => setName(event.target.value)}
                      maxLength={100}
                      placeholder="How should we call you?"
                    />
                  </Field>
                  <div>
                    <button className="button small secondary" disabled={busy}>
                      Save profile
                    </button>
                  </div>
                </form>
              </details>
            </Panel>
          )}
          {account && (
            <Panel
              title="Verified wallets"
              description="Each linked wallet has proved its ownership with a signature."
            >
              {account.wallets.map((wallet) => (
                <div
                  className="wallet-entry"
                  key={`${wallet.chain}:${wallet.network}:${wallet.address}`}
                >
                  <div className="wallet-identity">
                    <ChainBadge chain={wallet.chain} />
                    <Address value={wallet.address} />
                    {config &&
                      wallet.network !==
                        config.wallet_networks[wallet.chain] && (
                        <small className="muted">
                          Another deployment · link a wallet on this environment
                          to act here
                        </small>
                      )}
                  </div>
                  <span className="role-badge">
                    {wallet.chain === account.session.wallet.chain &&
                    wallet.network === account.session.wallet.network &&
                    sameAddress(
                      wallet.address,
                      account.session.wallet.address,
                      wallet.chain,
                    )
                      ? "Current session"
                      : "Verified"}
                  </span>
                </div>
              ))}
            </Panel>
          )}
          <Panel
            title={account ? "Link another wallet" : "Choose a network"}
            description={
              account
                ? "Add a wallet to this profile by signing a message with it."
                : "Choose the network of the wallet you want to use."
            }
          >
            <div className="panel-body form-stack">
              <div className="wallet-options">
                {chains.map((value) => (
                  <button
                    key={value}
                    type="button"
                    className={`wallet-choice ${chain === value ? "selected" : ""}`}
                    aria-pressed={chain === value}
                    disabled={busy}
                    onClick={() => setChain(value)}
                  >
                    <ChainBadge chain={value} />
                    <span>
                      {chain === value ? "Selected" : "Choose network"}
                    </span>
                  </button>
                ))}
              </div>
              {config && (
                <WalletPicker
                  chain={chain}
                  onConnect={authenticate}
                  onBusyChange={setBusy}
                  disabled={busy}
                />
              )}
              {config?.local_simulator &&
                ["localhost", "127.0.0.1", "[::1]"].includes(
                  location.hostname,
                ) && (
                  <details className="disclosure">
                    <summary>Use a local development wallet</summary>
                    <div className="disclosure-body">
                      <Notice>
                        Import a test wallet exported by your local environment.
                        Its key stays in memory in this tab and is cleared on
                        sign-out or reload. Use test keys only.
                      </Notice>
                      <Field label="Development wallet JSON">
                        <input
                          type="file"
                          accept="application/json,.json"
                          disabled={busy}
                          onChange={(event) => {
                            const file = event.target.files?.[0];
                            event.target.value = "";
                            if (file) void importLocal(file);
                          }}
                        />
                      </Field>
                    </div>
                  </details>
                )}
            </div>
          </Panel>
          {account && config && (
            <WalletHistory account={account} config={config} />
          )}
        </div>
        <Panel>
          <div className="panel-body wallet-explainer">
            <p className="eyebrow">Wallet access</p>
            <h3>Your keys. Your approval.</h3>
            <p>
              Signing in proves wallet ownership. You review and sign financial
              instructions separately.
            </p>
            <details className="disclosure">
              <summary>How project roles work</summary>
              <p>
                Owner and manager permissions come from project contracts.
                Linking a wallet does not grant a new role. Agents can request a
                budget; they receive funds only after approval and confirmed
                funding.
              </p>
            </details>
            <details className="disclosure">
              <summary>Which wallet signs each action?</summary>
              <p>
                Management decisions are signed on NEAR. Agent tasks and owner
                deposits use the wallet on their execution network. Each action
                shows the required wallet before you sign.
              </p>
            </details>
            <details className="disclosure">
              <summary>How do I keep one profile across networks?</summary>
              <p>
                Sign in with a wallet already linked to your profile, then use
                Link another wallet to add a new one. Signing in separately with
                an unlinked wallet creates a different profile.
              </p>
            </details>
          </div>
        </Panel>
      </div>
    </>
  );
}

function WalletHistory({
  account,
  config,
}: {
  account: Account;
  config: PublicConfig;
}) {
  const [checking, setChecking] = useState<string | null>(null);
  const [transactions, setTransactions] = useState(walletTransactions);
  const [recoveryHashes, setRecoveryHashes] = useState<Record<string, string>>(
    {},
  );
  const [error, setError] = useState<unknown>();
  const busy = checking !== null;
  useEffect(() => {
    const refresh = () => setTransactions(walletTransactions());
    window.addEventListener("storage", refresh);
    return () => window.removeEventListener("storage", refresh);
  }, []);
  async function recheck(transaction: WalletTransaction) {
    setChecking(transaction.id);
    setError(undefined);
    try {
      if (transaction.transactionId)
        await refreshWalletTransaction(transaction, config);
      else if (transaction.submission === "signed" || !transaction.nonceContext)
        discardUnsubmittedTransaction(transaction);
      else
        await resolveWalletTransaction(
          transaction,
          recoveryHashes[transaction.id] ?? "",
          config,
        );
    } catch (failure) {
      setError(failure);
    } finally {
      setTransactions(walletTransactions());
      setChecking(null);
    }
  }
  const accountTransactions = transactions.filter((transaction) =>
    account.wallets.some(
      (wallet) =>
        wallet.chain === transaction.chain &&
        sameAddress(wallet.address, transaction.actor, wallet.chain),
    ),
  );
  const unresolved = accountTransactions.filter((transaction) =>
    ["submitted", "unknown"].includes(transaction.status),
  ).length;
  const historyDetails = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    if (unresolved > 0 && historyDetails.current) {
      historyDetails.current.open = true;
    }
  }, [unresolved]);
  if (accountTransactions.length === 0) return null;
  return (
    <>
      {unresolved > 0 && (
        <Notice>
          <strong>
            {unresolved} wallet{" "}
            {unresolved === 1 ? "transaction needs" : "transactions need"}{" "}
            confirmation.
          </strong>
          <p>
            Check the transaction history below before submitting another action
            from the same wallet.
          </p>
          <a href="#wallet-transactions" className="text-link">
            Review wallet transactions ↓
          </a>
        </Notice>
      )}
      {error !== undefined && (
        <Notice tone="error">
          {error instanceof Error
            ? error.message
            : "Unable to check the transaction."}
        </Notice>
      )}

      <Panel
        title="Wallet transactions"
        description="Saved in this browser, including after a page reload."
      >
        <div id="wallet-transactions" />
        <details className="disclosure panel-disclosure" ref={historyDetails}>
          <summary>
            Transaction history{" "}
            <span className="disclosure-meta">
              {accountTransactions.length} recorded
              {unresolved > 0 ? ` · ${unresolved} need confirmation` : ""}
            </span>
          </summary>
          {[...accountTransactions]
            .sort(
              (a, b) =>
                Number(["submitted", "unknown"].includes(b.status)) -
                Number(["submitted", "unknown"].includes(a.status)),
            )
            .map((transaction) => (
              <div className="operation-card" key={transaction.id}>
                <div className="operation-title">
                  <ChainBadge chain={transaction.chain} />
                  <Status value={transaction.status} />
                </div>
                <p className="review-note">
                  Wallet <Address value={transaction.actor} />
                </p>
                {transaction.transactionId ? (
                  <Address value={transaction.transactionId} />
                ) : (
                  <p className="review-note">
                    {transaction.submission === "signed" ||
                    !transaction.nonceContext
                      ? "The wallet did not return signed bytes. The application has not broadcast this step."
                      : "The wallet did not return a transaction hash. It may have submitted the transaction; check the wallet’s activity history."}
                  </p>
                )}
                {config &&
                transaction.network !==
                  networkIdentity(transaction.chain, config) ? (
                  <p className="review-note">
                    Recorded on another deployment. Check this transaction using
                    its original network; it cannot be reconciled here.
                  </p>
                ) : (
                  ["submitted", "unknown"].includes(transaction.status) && (
                    <div className="form-stack" style={{ marginTop: 15 }}>
                      {!transaction.transactionId &&
                        transaction.submission === "wallet" &&
                        !!transaction.nonceContext && (
                          <Field
                            label="Transaction hash from wallet history"
                            hint="The chain transaction must match the original wallet, destination and exact action before it can resolve this request."
                          >
                            <input
                              value={recoveryHashes[transaction.id] ?? ""}
                              onChange={(event) =>
                                setRecoveryHashes((previous) => ({
                                  ...previous,
                                  [transaction.id]: event.target.value,
                                }))
                              }
                            />
                          </Field>
                        )}
                      <div className="operation-actions">
                        <button
                          className="button secondary small"
                          disabled={busy}
                          onClick={() => void recheck(transaction)}
                        >
                          {checking === transaction.id
                            ? "Checking…"
                            : !transaction.transactionId &&
                                (transaction.submission === "signed" ||
                                  !transaction.nonceContext)
                              ? "Dismiss unsubmitted request"
                              : "Check confirmation"}
                        </button>
                      </div>
                    </div>
                  )
                )}
              </div>
            ))}
        </details>
      </Panel>
    </>
  );
}

export function ProjectsPage() {
  const { account } = useSession();
  const result = useLoad(
    `projects:${account?.session.profile.id}`,
    api.projects,
  );
  return (
    <>
      <PageTitle
        eyebrow="Your workspace"
        title="Projects"
        action={
          <button
            className="button secondary small"
            disabled={result.loading}
            onClick={result.refresh}
          >
            Refresh
          </button>
        }
      >
        Open a project to review its budget, manage tasks and follow funding.
      </PageTitle>
      <p className="page-help">
        <GuideLink>New here? Find your next step →</GuideLink>
      </p>
      {result.error !== undefined && (
        <ErrorState error={result.error} retry={result.refresh} />
      )}
      {result.loading && !result.data && <Loading />}
      {result.data?.length === 0 && (
        <Panel>
          <Empty
            title="No projects are available yet"
            action={
              <GuideLink section="owner">
                How projects become available →
              </GuideLink>
            }
          >
            Your profile is ready. Ask the person managing this Apodect
            deployment to connect your project, then refresh this page.
          </Empty>
        </Panel>
      )}
      {result.data && (
        <div className="panel-grid">
          {result.data.map((project) => (
            <Link
              key={project.key}
              className="project-card"
              to={`/app/projects/${encodeURIComponent(project.key)}`}
            >
              <div
                className="button-row"
                style={{ justifyContent: "space-between" }}
              >
                <span className="project-icon" aria-hidden="true">
                  ◈
                </span>
                <RoleBadges roles={project.roles} />
              </div>
              <h2>{short(project.coordinator, 14)}</h2>
              <p>
                {project.indexed
                  ? `${project.task_count} tasks · Ethereum, Solana & NEAR`
                  : "Waiting for the first confirmed chain snapshot"}
              </p>
              <div className="project-card-footer">
                <div>
                  <p>Available to fund</p>
                  {project.indexed ? (
                    <Money value={project.available} />
                  ) : (
                    <span className="muted">Not yet observed</span>
                  )}
                </div>
                <span aria-hidden="true">↗</span>
              </div>
              {project.attention_count > 0 && (
                <p style={{ marginTop: 15 }}>
                  {project.attention_count} funding{" "}
                  {project.attention_count === 1 ? "plan needs" : "plans need"}{" "}
                  attention
                </p>
              )}
            </Link>
          ))}
        </div>
      )}
    </>
  );
}
