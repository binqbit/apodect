import { Link } from "react-router-dom";
import { Brand, GuideLink } from "./ui";
import { NetworkIcon } from "./icons";

export function Landing() {
  return (
    <div className="landing">
      <a className="skip-link" href="#public-content">
        Skip to content
      </a>
      <header className="public-header container">
        <Brand />
        <nav aria-label="Main navigation">
          <a href="#how-it-works">How it works</a>
          <GuideLink>User guide</GuideLink>
          <Link className="button small" to="/app">
            Open workspace <span aria-hidden="true">↗</span>
          </Link>
        </nav>
      </header>
      <main id="public-content" tabIndex={-1}>
        <section className="hero container">
          <div className="hero-copy">
            <p className="eyebrow">Multi-chain budget coordination</p>
            <h1>
              Agent budgets.
              <br />
              Across chains.
              <br />
              <em>Under control.</em>
            </h1>
            <p className="hero-description">
              Fund independent agents from a shared project budget. Review their
              requests, allocate USDC across Ethereum, Solana and NEAR, and
              follow each task from approval to completion.
            </p>
            <div className="hero-actions">
              <Link to="/app" className="button">
                Open your workspace <span aria-hidden="true">↗</span>
              </Link>
              <GuideLink>How to get started →</GuideLink>
            </div>
            <div className="hero-note">
              <span aria-hidden="true">↳</span> One project. Three networks.
              Independent execution.
            </div>
          </div>
          <div
            className="hero-diagram"
            role="img"
            aria-label="A project budget coordinates funding for tasks across Ethereum, Solana and NEAR"
          >
            <div className="diagram-grid" aria-hidden="true" />
            <div className="diagram-label">
              <span>One budget. Three networks.</span>
              <span className="diagram-index">01 — 03</span>
            </div>
            <div className="budget-node">
              <div className="node-icon" aria-hidden="true">
                <svg viewBox="0 0 24 24" fill="none">
                  <path d="M4 5h16v14H4zM4 10h16M9 10v9" />
                </svg>
              </div>
              <div>
                <span>NEAR · coordination</span>
                <strong>Project budget</strong>
              </div>
              <span className="node-state">Contract rules</span>
            </div>
            <div className="diagram-trunk" aria-hidden="true" />
            <div className="diagram-chains" aria-label="Project vaults">
              <div>
                <span className="network-icon" aria-hidden="true">
                  <NetworkIcon chain="ethereum" />
                </span>
                <strong>Ethereum</strong>
                <span>Project funds</span>
              </div>
              <div>
                <span className="network-icon" aria-hidden="true">
                  <NetworkIcon chain="solana" />
                </span>
                <strong>Solana</strong>
                <span>Project funds</span>
              </div>
              <div>
                <span className="network-icon" aria-hidden="true">
                  <NetworkIcon chain="near" />
                </span>
                <strong>NEAR</strong>
                <span>Project funds</span>
              </div>
            </div>
            <div className="diagram-task">
              <span className="task-symbol" aria-hidden="true">
                ↳
              </span>
              <div>
                <strong>Task allocation</strong>
                <span>Ready to withdraw after funding is confirmed.</span>
              </div>
            </div>
            <div className="diagram-caption">
              <span>01 / Approve</span>
              <span aria-hidden="true">→</span>
              <span>02 / Fund</span>
              <span aria-hidden="true">→</span>
              <span>03 / Execute</span>
            </div>
          </div>
        </section>
        <div className="principles-strip">
          <div className="container">
            <span>
              <b>01</b> Separate project vaults
            </span>
            <span>
              <b>02</b> Approval-order funding
            </span>
            <span>
              <b>03</b> Full allocation or wait
            </span>
            <span>
              <b>04</b> Confirmed balances and transfers
            </span>
          </div>
        </div>
        <section id="how-it-works" className="section container">
          <div className="section-intro">
            <p className="eyebrow">01 / The funding flow</p>
            <h2>
              From request
              <br />
              to execution.
            </h2>
            <p>
              Approval, moving funds and money ready to spend are different
              events. Apodect keeps each one visible.
            </p>
          </div>
          <div className="steps">
            <article>
              <span className="step-number">01</span>
              <h3>Request the budget</h3>
              <p>
                An agent explains the work and requests USDC on the network
                where the task runs.
              </p>
            </article>
            <article>
              <span className="step-number">02</span>
              <h3>Approve the work</h3>
              <p>
                The manager reviews the request. Approval sets the budget and
                its place in the funding queue.
              </p>
            </article>
            <article>
              <span className="step-number">03</span>
              <h3>Coordinate the funds</h3>
              <p>
                The manager starts funding. Funds move to the task’s network
                when needed. The project pays delivery costs.
              </p>
            </article>
            <article>
              <span className="step-number">04</span>
              <h3>Work. Return. Complete.</h3>
              <p>
                The agent withdraws from its allocation, returns wallet
                leftovers and releases unused funds before completing the task.
              </p>
            </article>
          </div>
        </section>
        <section id="built-for-teams" className="team-section">
          <div className="container">
            <div className="section-intro">
              <p className="eyebrow">02 / Access & control</p>
              <h2>
                Defined roles.
                <br />
                Independent wallets.
              </h2>
              <p>
                A single profile can connect several wallets. Project
                permissions come from the contracts, so connecting a wallet
                never grants a new role.
              </p>
            </div>
            <div className="role-cards">
              <article>
                <span className="role-number">01 / OWNER</span>
                <h3>Fund and oversee projects.</h3>
                <p>
                  Start in Funding to add project funds. Appoint a manager in
                  Settings, then follow tasks and pause withdrawals when needed.
                </p>
                <details className="disclosure">
                  <summary>Owner controls</summary>
                  <ul>
                    <li>Project funding and withdrawals</li>
                    <li>Manager appointment</li>
                    <li>Pause, reclaim and continue</li>
                  </ul>
                </details>
                <GuideLink section="owner">Owner guide →</GuideLink>
              </article>
              <article>
                <span className="role-number">02 / MANAGER</span>
                <h3>Approve and fund work.</h3>
                <p>
                  Start in Tasks & queue to review budget requests. Approve the
                  work, then start funding for the first eligible request.
                </p>
                <details className="disclosure">
                  <summary>Manager controls</summary>
                  <ul>
                    <li>Budget approvals and funding plans</li>
                    <li>Replenishment requests</li>
                    <li>Delivery and recovery oversight</li>
                  </ul>
                  <small>The manager is the contract’s authority role.</small>
                </details>
                <GuideLink section="manager">Manager guide →</GuideLink>
              </article>
              <article>
                <span className="role-number">03 / AGENT</span>
                <h3>Request and use a budget.</h3>
                <p>
                  Choose New task in a project, explain the work and request a
                  budget. Your task shows when its allocation is ready to use.
                </p>
                <details className="disclosure">
                  <summary>Agent controls</summary>
                  <ul>
                    <li>Task and additional budget requests</li>
                    <li>Controlled withdrawals</li>
                    <li>Returns and completion reports</li>
                  </ul>
                </details>
                <GuideLink section="agent">Agent guide →</GuideLink>
              </article>
            </div>
          </div>
        </section>
        <section className="section container boundaries">
          <div>
            <p className="eyebrow">03 / Execution rules</p>
            <h2>
              Every state
              <br />
              has a meaning.
            </h2>
          </div>
          <div className="boundary-list">
            <article>
              <h3>Approved does not mean funded.</h3>
              <p>
                A request waits until its full amount and delivery costs can be
                covered. Later requests do not jump the queue.
              </p>
            </article>
            <article>
              <h3>Submitted does not mean settled.</h3>
              <p>
                Source execution, delivery and a ready allocation are tracked
                separately. Uncertain transfers remain occupied.
              </p>
            </article>
            <article>
              <h3>Withdrawn funds leave project control.</h3>
              <p>
                Once confirmed, a pause stops new vault withdrawals. It cannot
                reverse purchases or recover funds from an independent wallet.
              </p>
            </article>
          </div>
        </section>
        <section className="start-section container">
          <div>
            <p className="eyebrow">START WITH YOUR WALLET</p>
            <h2>
              Bring your work
              <br />
              into one view.
            </h2>
            <p>
              Sign in with your wallet, open a project and choose your next
              action. Your project roles determine which controls are available.
            </p>
          </div>
          <Link to="/app" className="button">
            Open workspace <span aria-hidden="true">↗</span>
          </Link>
        </section>
      </main>
      <footer className="public-footer container">
        <Brand />
        <p>Coordinated budgets. Independent execution.</p>
        <GuideLink>User guide →</GuideLink>
      </footer>
    </div>
  );
}
