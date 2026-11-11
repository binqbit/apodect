import { useEffect, useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { api, type Chain, type ProjectRoles } from "./api";
import { useSession } from "./app";
import {
  connectWallet,
  discoverWallets,
  importDevelopmentWallet,
  refreshWalletTransaction,
  resolveWalletTransaction,
  discardUnsubmittedTransaction,
  walletTransactions,
  sameAddress,
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
          <span className="role-badge">Connected participant</span>
        )}
    </div>
  );
}

export function WalletPicker({
  chain,
  onConnect,
  disabled = false,
}: {
  chain: Chain;
  onConnect: (wallet: ConnectedWallet) => void | Promise<void>;
  disabled?: boolean;
}) {
  const { config, rememberWallet } = useSession();
  const [options, setOptions] = useState<WalletOption[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
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
    setError(undefined);
    try {
      const wallet = await connectWallet(option, config);
      rememberWallet(wallet);
      await onConnect(wallet);
    } catch (failure) {
      setError(failure);
    } finally {
      setBusy(false);
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
      <div className="button-row">
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
  const {
    account,
    config,
    loading,
    error: sessionError,
    setAccount,
    refreshAccount,
    rememberWallet,
  } = useSession();
  const [chain, setChain] = useState<Chain>("ethereum");
  const [name, setName] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  const [transactions, setTransactions] = useState(walletTransactions);
  const [recoveryHashes, setRecoveryHashes] = useState<Record<string, string>>(
    {},
  );
  useEffect(
    () => setName(account?.session.profile.display_name ?? ""),
    [account?.session.profile.display_name],
  );
  async function authenticate(wallet: ConnectedWallet) {
    setMessage("");
    setError(undefined);
    const challenge = account
      ? await api.linkChallenge(
          wallet.chain,
          wallet.address,
          wallet.callbackUrl,
        )
      : await api.challenge(wallet.chain, wallet.address, wallet.callbackUrl);
    const signature = await wallet.signChallenge(challenge);
    const proof = { ...signature, challenge: challenge.id };
    setAccount(account ? await api.linkWallet(proof) : await api.login(proof));
    setMessage(
      account
        ? "Wallet ownership verified. The wallet is now linked to this profile."
        : "Wallet verified. Your workspace is ready.",
    );
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
  async function recheck(transaction: WalletTransaction) {
    if (!config) return;
    setBusy(true);
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
        title={account ? "Account & wallets" : "One profile. Your own wallets."}
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
          : "Connect a wallet and sign a message to access your projects. No transaction or token approval is needed to sign in."}
      </PageTitle>
      {message && (
        <Notice tone="success">
          {message}
          {account && (
            <p>
              <Link className="text-link" to="/app">
                Go to projects →
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
            <Panel title="Profile">
              <form
                className="panel-body form-stack"
                onSubmit={(event) => void rename(event)}
              >
                <Field
                  label="Display name"
                  hint="Optional. Your name does not change your contract permissions."
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
                  <div>
                    <ChainBadge chain={wallet.chain} />
                    <Address value={wallet.address} />
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
            title={account ? "Link another wallet" : "Connect your wallet"}
            description={
              account
                ? "Sign with the new wallet to link it to this profile."
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
                <WalletPicker chain={chain} onConnect={authenticate} />
              )}
              {config?.local_simulator &&
                ["localhost", "127.0.0.1", "[::1]"].includes(
                  location.hostname,
                ) && (
                  <details>
                    <summary>Use a local development wallet</summary>
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
                  </details>
                )}
            </div>
          </Panel>
          {account && transactions.length > 0 && (
            <Panel
              title="Wallet transactions"
              description="Submitted transactions survive a page reload. Check uncertain results before starting another payment."
            >
              {transactions
                .filter((transaction) =>
                  account.wallets.some(
                    (wallet) =>
                      wallet.chain === transaction.chain &&
                      sameAddress(
                        wallet.address,
                        transaction.actor,
                        wallet.chain,
                      ),
                  ),
                )
                .map((transaction) => (
                  <div className="operation-card" key={transaction.id}>
                    <div className="operation-title">
                      <ChainBadge chain={transaction.chain} />
                      <Status value={transaction.status} />
                    </div>
                    {transaction.transactionId ? (
                      <code>{transaction.transactionId}</code>
                    ) : (
                      <p className="review-note">
                        {transaction.submission === "signed" ||
                        !transaction.nonceContext
                          ? "The wallet did not return signed bytes. The application has not broadcast this step."
                          : "The wallet did not return a transaction hash. It may have submitted the transaction; check the wallet’s activity history."}
                      </p>
                    )}
                    {["submitted", "unknown"].includes(transaction.status) && (
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
                            {busy
                              ? "Checking…"
                              : !transaction.transactionId &&
                                  (transaction.submission === "signed" ||
                                    !transaction.nonceContext)
                                ? "Dismiss unsubmitted request"
                                : "Check confirmation"}
                          </button>
                        </div>
                      </div>
                    )}
                  </div>
                ))}
            </Panel>
          )}
        </div>
        <Panel>
          <div className="panel-body wallet-explainer">
            <h3>A profile is not a role.</h3>
            <p>
              Owner and manager permissions are read from each project’s
              contracts. An agent can create a request, but funds become
              available only after approval and confirmed funding.
            </p>
            <h3>Use the right wallet for each action.</h3>
            <p>
              Management decisions are signed on NEAR. Agent tasks and owner
              deposits use the wallet on the task or funding network.
            </p>
            <h3>Your keys stay with you.</h3>
            <p>
              Apodect prepares transactions for your review. The backend never
              signs as an owner, manager or agent.
            </p>
          </div>
        </Panel>
      </div>
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
        Separate budgets. A shared view of work across networks.
      </PageTitle>
      {result.error !== undefined && (
        <ErrorState error={result.error} retry={result.refresh} />
      )}
      {result.loading && !result.data && <Loading />}
      {result.data?.length === 0 && (
        <Panel>
          <Empty title="No projects are available yet">
            Project deployments appear here once connected to this platform.
            Your wallet profile is ready.
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
                  <p>Eligible for funding</p>
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
