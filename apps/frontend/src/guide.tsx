import { useEffect } from "react";
import { Link, useLocation } from "react-router-dom";
import { Brand } from "./ui";

const sections = [
  ["owner", "I fund the project"],
  ["manager", "I manage budgets"],
  ["agent", "I carry out tasks"],
  ["funding", "Understand funding"],
  ["wallets", "Wallets & signing"],
] as const;

export function Guide() {
  const location = useLocation();
  const requested: unknown = location.state?.returnTo;
  const returnTo =
    typeof requested === "string" &&
    (requested === "/" ||
      requested === "/app" ||
      requested === "/app/account" ||
      requested.startsWith("/app/projects/"))
      ? requested
      : "/app";
  const returnLabel = returnTo.startsWith("/app/projects/")
    ? "Back to project"
    : returnTo === "/app/account"
      ? "Back to account"
      : returnTo === "/"
        ? "Back to home"
        : "Back to workspace";

  useEffect(() => {
    document.title = "User guide · Apodect";
    const frame = requestAnimationFrame(() => {
      const section = sections.find(([id]) => `#${id}` === location.hash);
      const target = document.getElementById(section?.[0] ?? "guide-content");
      target?.focus({ preventScroll: true });
      if (section) target?.scrollIntoView({ block: "start" });
      else window.scrollTo(0, 0);
    });
    return () => cancelAnimationFrame(frame);
  }, [location.hash]);

  return (
    <div className="guide-page">
      <a className="skip-link" href="#guide-content">
        Skip to content
      </a>
      <header className="public-header container">
        <Brand />
        <Link
          to={returnTo}
          state={location.state?.returnState}
          className="button small secondary"
        >
          ← {returnLabel}
        </Link>
      </header>
      <main
        className="container guide-content"
        id="guide-content"
        tabIndex={-1}
      >
        <div className="guide-intro">
          <p className="eyebrow">Apodect / User guide</p>
          <h1>Know what to do next.</h1>
          <p className="muted">
            Apodect helps teams fund agent tasks in USDC across Ethereum, Solana
            and NEAR. Owners supply funds, managers approve budgets, and agents
            withdraw their confirmed allocations to do the work.
          </p>
        </div>
        <div className="guide-start">
          <strong>Start with your wallet</strong>
          <p>
            Open the workspace, choose a network and sign in. Your first sign-in
            creates a profile. Open a project, then follow the guide for your
            role below. One person can have several roles.
          </p>
          <Link className="text-link" to="/app">
            Open workspace →
          </Link>
        </div>
        <nav className="guide-nav" aria-label="Guide topics">
          {sections.map(([id, label]) => (
            <Link
              key={id}
              to={`/guide#${id}`}
              state={{ returnTo, returnState: location.state?.returnState }}
            >
              {label} <span aria-hidden="true">↓</span>
            </Link>
          ))}
        </nav>
        <div className="guide-roles">
          <section className="guide-role" id="owner" tabIndex={-1}>
            <p className="eyebrow">Owner</p>
            <h2>Fund and oversee a project</h2>
            <ol>
              <li>
                <strong>Add funds.</strong> Open Funding, choose a network and
                select Add funds. Wait for the deposit to be confirmed and
                synchronized.
              </li>
              <li>
                <strong>Choose a manager.</strong> Settings shows the manager
                who reviews requests. Replace manager changes that authority;
                signing in does not assign it.
              </li>
              <li>
                <strong>Monitor the work.</strong> Overview shows the budget and
                Tasks & queue shows individual tasks. Pause withdrawals when
                necessary; pausing does not retrieve money already in an agent’s
                wallet.
              </li>
            </ol>
            <details className="disclosure">
              <summary>My project is missing</summary>
              <p>
                Ask the deployment operator to connect the project to this
                workspace. This version manages already deployed projects; it
                does not create or deploy a new project from the website. For a
                connected project, link the wallet that holds the owner role.
              </p>
            </details>
          </section>
          <section className="guide-role" id="manager" tabIndex={-1}>
            <p className="eyebrow">Manager</p>
            <h2>Turn requests into funded work</h2>
            <ol>
              <li>
                <strong>Review the request.</strong> In Tasks & queue, filter
                Awaiting approval, open the task and check its purpose and
                amount before approving or rejecting.
              </li>
              <li>
                <strong>Start funding separately.</strong> Review funding for
                the first eligible request in the queue. Check the full
                allocation and delivery costs, then sign. Approval alone sends
                no funds.
              </li>
              <li>
                <strong>Follow confirmation.</strong> Activity shows funding
                progress. If the project needs more funds, request them from the
                owner in Funding. Later requests cannot skip the first waiting
                request.
              </li>
            </ol>
            <details className="disclosure">
              <summary>What does a funding request to the owner do?</summary>
              <p>
                It asks for replenishment; it does not move money. The owner’s
                response closes that request, including a partial response.
                Funds become usable only after an actual deposit is confirmed.
              </p>
            </details>
          </section>
          <section className="guide-role" id="agent" tabIndex={-1}>
            <p className="eyebrow">Agent</p>
            <h2>Request, work, return, complete</h2>
            <ol>
              <li>
                <strong>Create your task.</strong> Select New task, choose its
                execution network, explain the work and request a USDC budget.
                You sign with your own wallet.
              </li>
              <li>
                <strong>Wait for an allocation.</strong> The manager approves
                and starts funding. Withdraw only when the task has a confirmed
                available allocation and withdrawals are permitted.
              </li>
              <li>
                <strong>Finish with the remainder.</strong> Return wallet funds
                sends unused USDC back from your wallet. Release allocation
                frees unused funds still held for your task in the vault. Then
                complete the task.
              </li>
            </ol>
            <details className="disclosure">
              <summary>I need more budget or my task is paused</summary>
              <p>
                Request budget creates a new request for the manager; it needs a
                new approval and queue position. A pause blocks withdrawals but
                still allows returns. Only the owner can continue a paused task,
                once its available allocation and pending transfers permit it.
              </p>
            </details>
          </section>
        </div>
        <section className="guide-section" id="funding" tabIndex={-1}>
          <p className="eyebrow">The funding flow</p>
          <h2>Approved does not yet mean spendable.</h2>
          <div className="guide-milestones">
            <div>
              <strong>1. Approved</strong>
              <p>
                The amount and approval-order priority are set. The manager
                still needs to start funding.
              </p>
            </div>
            <div>
              <strong>2. Funding</strong>
              <p>
                Funds are being reserved, moved or assigned. In-transit and
                committed funds cannot be reused.
              </p>
            </div>
            <div>
              <strong>3. Allocated</strong>
              <p>
                The full amount is confirmed in the task’s vault allocation. An
                active, unpaused task can withdraw it.
              </p>
            </div>
          </div>
          <details className="disclosure">
            <summary>Why is an approved request waiting?</summary>
            <p>
              The manager may not have started funding, an earlier approved
              request may still block the queue, or available funds may not
              cover the full amount plus delivery costs. Check Tasks & queue
              first, then Activity for a funding plan already in progress.
              Approval never becomes a smaller partial allocation.
            </p>
          </details>
          <details className="disclosure">
            <summary>What do the different balances mean?</summary>
            <p>
              Available project funds can support new allocations. A task
              allocation is reserved for that task inside a vault. After
              withdrawal, funds are in the agent’s independent wallet. Returning
              funds or releasing an allocation makes them available to the
              project; neither refills the task’s allowance.
            </p>
          </details>
          <details className="disclosure">
            <summary>Who pays the fees, and what can be recovered?</summary>
            <p>
              The project pays delivery costs separately from the approved agent
              amount. For example, 60 USDC allocated plus 1 USDC in delivery
              costs uses 61 USDC of project funds; actual costs appear in the
              funding review. Wallets also need the network’s native token for
              gas, rent or storage. Apodect controls funds in its vaults; it
              cannot automatically recover funds withdrawn to independent
              wallets.
            </p>
          </details>
        </section>
        <section className="guide-section" id="wallets" tabIndex={-1}>
          <p className="eyebrow">Access & confirmation</p>
          <h2>Your profile, wallets and signatures.</h2>
          <details className="disclosure">
            <summary>Which wallet should I use?</summary>
            <p>
              Use the wallet assigned to your role. Manager coordination uses
              NEAR; deposits and agent withdrawals use a wallet on the network
              where the action happens. Account & wallets lets you link
              additional wallets after proving ownership. Linking a wallet does
              not create an owner or manager permission.
            </p>
          </details>
          <details className="disclosure">
            <summary>My wallet is not listed</summary>
            <p>
              Enable a compatible wallet extension, or open the site in its
              wallet browser, then reload. Select the intended account and
              deployment network. If you use a different wallet, sign out before
              signing in to that profile, or link it from your existing profile.
            </p>
          </details>
          <details className="disclosure">
            <summary>What am I signing?</summary>
            <p>
              Signing in proves wallet ownership. An operation is separate:
              enter its details, review the project, amount, network, signer and
              costs, then approve in your wallet. You can edit or close the
              review before signing. Some operations require several
              transactions in order.
            </p>
          </details>
          <details className="disclosure">
            <summary>
              I signed, but the result is still pending or unknown
            </summary>
            <p>
              Submission is not completion. Keep the operation’s transaction
              reference and use Account & wallets → Wallet transactions →
              Transaction history to check its status. Do not submit a
              replacement while the result is unknown. A confirmed transaction
              may still need synchronization or cross-chain delivery; check the
              project’s Activity for that progress.
            </p>
          </details>
        </section>
        <div className="guide-return">
          <p className="muted">Ready to continue?</p>
          <Link
            className="button secondary"
            to={returnTo}
            state={location.state?.returnState}
          >
            ← {returnLabel}
          </Link>
        </div>
      </main>
    </div>
  );
}
