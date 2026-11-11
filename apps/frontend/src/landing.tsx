import { Link } from "react-router-dom";
import { Brand } from "./ui";

export function Landing() {
  return (
    <div className="landing">
      <header className="public-header container">
        <Brand />
        <nav aria-label="Main navigation">
          <a href="#how-it-works">How it works</a>
          <a href="#built-for-teams">For your team</a>
          <Link className="button small" to="/app">
            Open workspace <span aria-hidden="true">↗</span>
          </Link>
        </nav>
      </header>
      <main>
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
              Fund the work. Coordinate the liquidity. Track every allocation
              across Ethereum, Solana and NEAR — with permissions enforced by
              contracts and actions signed by your wallet.
            </p>
            <div className="hero-actions">
              <Link to="/app" className="button">
                Open your workspace <span aria-hidden="true">↗</span>
              </Link>
              <a href="#how-it-works" className="text-link">
                See how it works <span aria-hidden="true">↓</span>
              </a>
            </div>
            <div className="hero-note">
              <span aria-hidden="true">↳</span> One project. Three networks.
              Independent execution.
            </div>
          </div>
          <div
            className="hero-diagram"
            aria-label="A project budget coordinates funding for tasks across Ethereum, Solana and NEAR"
          >
            <div className="diagram-grid" aria-hidden="true" />
            <div className="diagram-label">
              <span>Funding architecture</span>
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
                  <svg viewBox="0 0 24 24" fill="none">
                    <path d="m12 2 6 10-6 4-6-4 6-10ZM6 15l6 7 6-7-6 4-6-4Z" />
                  </svg>
                </span>
                <strong>Ethereum</strong>
                <span>Local vault</span>
              </div>
              <div>
                <span className="network-icon" aria-hidden="true">
                  <svg viewBox="0 0 24 24" fill="none">
                    <path d="m6 5 2-2h12l-2 2H6Zm0 6h12l2 2H8l-2-2Zm0 10 2-2h12l-2 2H6Z" />
                  </svg>
                </span>
                <strong>Solana</strong>
                <span>Local vault</span>
              </div>
              <div>
                <span className="network-icon" aria-hidden="true">
                  N
                </span>
                <strong>NEAR</strong>
                <span>Local vault</span>
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
              <b>04</b> Confirmed chain evidence
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
                An agent creates a task on the network where it works, explains
                the purpose and requests USDC.
              </p>
            </article>
            <article>
              <span className="step-number">02</span>
              <h3>Approve the work</h3>
              <p>
                The project manager reviews requests. Approval fixes the amount
                and its place in the funding queue.
              </p>
            </article>
            <article>
              <span className="step-number">03</span>
              <h3>Coordinate the funds</h3>
              <p>
                The manager starts funding. Available liquidity is allocated
                locally or moved across networks, with delivery costs paid by
                the project.
              </p>
            </article>
            <article>
              <span className="step-number">04</span>
              <h3>Work. Return. Complete.</h3>
              <p>
                The agent withdraws from its confirmed allocation, returns the
                remainder and completes the task.
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
                <h3>Set the direction.</h3>
                <p>
                  Fund the project, appoint its manager and keep control over
                  unused funds. Pause a task when its next step needs a
                  decision.
                </p>
                <ul>
                  <li>Project funding and withdrawals</li>
                  <li>Manager appointment</li>
                  <li>Pause, reclaim and continue</li>
                </ul>
              </article>
              <article>
                <span className="role-number">02 / MANAGER</span>
                <h3>Keep work moving.</h3>
                <p>
                  Review budgets, follow approval priority and coordinate
                  liquidity. See what is ready, waiting or needs attention.
                </p>
                <ul>
                  <li>Budget approvals and funding plans</li>
                  <li>Replenishment requests</li>
                  <li>Delivery and recovery oversight</li>
                </ul>
                <small>The manager is the contract’s authority role.</small>
              </article>
              <article>
                <span className="role-number">03 / AGENT</span>
                <h3>Focus on the task.</h3>
                <p>
                  Request funds where the work happens. Use a confirmed
                  allocation through your own wallet and account for what
                  remains.
                </p>
                <ul>
                  <li>Task and additional budget requests</li>
                  <li>Controlled withdrawals</li>
                  <li>Returns and completion reports</li>
                </ul>
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
              <h3>Wallet funds belong to the agent.</h3>
              <p>
                Once withdrawn, funds are independently controlled. A pause
                stops new withdrawals; it does not reverse purchases or seize a
                wallet.
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
              Connect a wallet to see your projects and the actions you can
              take.
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
        <a href="#how-it-works">How it works ↑</a>
      </footer>
    </div>
  );
}
