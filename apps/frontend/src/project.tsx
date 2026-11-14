import { useEffect, useRef, useState } from "react";
import { Link, NavLink, useLocation, useParams } from "react-router-dom";
import { api, type ProjectDetail } from "./api";
import { ActionDialog, type ActionTarget } from "./action";
import { RoleBadges } from "./pages";
import { Tasks, currentRequest, taskStatus } from "./tasks";
import { Funding } from "./funding";
import { Activity } from "./activity";
import { Icon } from "./icons";
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
  ReferenceContent,
  Status,
  chainNames,
  chains,
  short,
  useLoad,
} from "./ui";

export function ProjectPage() {
  const { projectKey = "" } = useParams();
  const location = useLocation();
  const result = useLoad(projectKey, () => api.project(projectKey));
  const [action, setAction] = useState<ActionTarget | null>(null);
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (!document.hidden) result.refresh();
    }, 7000);
    return () => window.clearInterval(timer);
  }, [projectKey]);
  useEffect(() => setAction(null), [projectKey]);
  const tab = location.pathname.split("/")[4] ?? "overview";
  const project = result.data;
  const base = `/app/projects/${encodeURIComponent(projectKey)}`;
  if (!project)
    return result.error ? (
      <ErrorState error={result.error} retry={result.refresh} />
    ) : (
      <Loading />
    );
  return (
    <>
      <PageTitle
        eyebrow="Project workspace"
        title={project.name}
        action={
          <div className="button-row">
            <Link
              to={`${base}/settings#invite`}
              className="button secondary small"
            >
              <Icon name="link" />
              Invite people
            </Link>
            <button
              className="button secondary small"
              disabled={result.loading}
              onClick={result.refresh}
            >
              <Icon name="refresh" />
              Refresh
            </button>
            <button
              className="button small"
              onClick={() => setAction({ kind: "create_task" })}
            >
              <Icon name="plus" />
              New task
            </button>
          </div>
        }
      >
        <span className="inline-meta">
          <span>
            Updated{" "}
            {project.observed_at
              ? new Date(project.observed_at).toLocaleString()
              : "not yet observed"}
          </span>
        </span>
      </PageTitle>
      <ProjectAccess project={project} />
      {result.error !== undefined && (
        <Notice tone="error">
          Refresh failed. Displaying the last observed state; available balances
          may have changed.
          {result.error instanceof Error && (
            <details className="disclosure">
              <summary>Refresh error details</summary>
              <p>{result.error.message}</p>
            </details>
          )}
        </Notice>
      )}
      {!project.indexed && (
        <Notice>
          Network data is incomplete. Balances and tasks may be missing until
          the remaining networks are confirmed.
        </Notice>
      )}
      <nav className="tabs" aria-label="Project sections">
        <NavLink end to={base}>
          Overview
        </NavLink>
        <NavLink to={`${base}/tasks`}>Tasks & queue</NavLink>
        <NavLink to={`${base}/funding`}>Funding</NavLink>
        <NavLink to={`${base}/activity`}>Activity</NavLink>
        <NavLink to={`${base}/settings`}>Settings</NavLink>
      </nav>
      {tab === "overview" && <Overview project={project} base={base} />}
      {tab === "tasks" && (
        <Tasks key={project.key} project={project} act={setAction} />
      )}
      {tab === "funding" && (
        <Funding key={project.key} project={project} act={setAction} />
      )}
      {tab === "activity" && (
        <Activity key={project.key} project={project} act={setAction} />
      )}
      {tab === "settings" && (
        <Settings key={project.key} project={project} act={setAction} />
      )}
      {!["overview", "tasks", "funding", "activity", "settings"].includes(
        tab,
      ) && (
        <Empty
          title="This project page does not exist"
          action={
            <Link to={base} className="button secondary">
              Project overview
            </Link>
          }
        />
      )}
      {action && (
        <ActionDialog
          key={`${action.kind}:${action.task?.id ?? action.request?.id ?? action.plan?.id ?? action.invoice?.id ?? ""}`}
          project={project}
          target={action}
          onClose={() => setAction(null)}
          onSubmitted={result.refresh}
        />
      )}
    </>
  );
}

function ProjectAccess({ project }: { project: ProjectDetail }) {
  const { roles } = project;
  const ownedNetworks = chains.filter((chain) => roles.local_owners[chain]);
  const participant = !roles.owner && !roles.authority && !roles.agent;
  return (
    <section className="project-access" aria-labelledby="project-role-title">
      <div className="project-access-heading">
        <p className="project-access-label" id="project-role-title">
          Your role in this project
        </p>
        <RoleBadges roles={roles} />
      </div>
      <div className="project-access-capabilities">
        {roles.owner && (
          <p>Control project funding and choose its single manager.</p>
        )}
        {roles.authority && (
          <p>Review budget requests and coordinate funding in queue order.</p>
        )}
        {roles.agent && (
          <p>
            Manage your own tasks, withdraw confirmed allocation and return
            unused funds.
          </p>
        )}
        {ownedNetworks.length > 0 && (
          <p>
            You own the vaults on{" "}
            {ownedNetworks.map((chain) => chainNames[chain]).join(", ")} and can
            add funds from their registered owner wallets.
          </p>
        )}
        {participant && (
          <p>
            You can create a task and request a budget. Owner or manager
            permission is required to manage the project.
          </p>
        )}
      </div>
    </section>
  );
}

function Overview({ project, base }: { project: ProjectDetail; base: string }) {
  const allocation = project.tasks
    .reduce((sum, task) => sum + BigInt(task.available), 0n)
    .toString();
  const attention = project.plans.filter(
    (plan) =>
      plan.requires_attention && !["funded", "closed"].includes(plan.status),
  );
  const openTasks = project.tasks.filter(
    (task) => !["completed", "canceled"].includes(task.state),
  );
  const awaitingApproval = project.requests.filter(
    (request) =>
      request.status === "pending" &&
      currentRequest(
        project.tasks.find(
          (task) => task.chain === request.chain && task.id === request.task,
        ),
        request,
      ),
  ).length;
  const nextFunding = project.requests.find(
    (request) =>
      request.queue_head && request.status === "queued" && !request.plan,
  );
  return (
    <>
      {attention.length > 0 && (
        <Notice>
          <strong>
            {attention.length} funding{" "}
            {attention.length === 1 ? "plan needs" : "plans need"} attention.
          </strong>
          <p>
            Existing transfers continue to be checked. Review the confirmed
            state before choosing a recovery action.{" "}
            <Link className="text-link" to={`${base}/activity`}>
              View activity →
            </Link>
          </p>
        </Notice>
      )}
      <div className="stats overview-stats">
        <div className="stat stat-primary">
          <p>Available to fund</p>
          <strong>
            <Money value={project.available} unit={false} />
          </strong>
          <small>USDC · Existing commitments excluded</small>
        </div>
        <div className="stat">
          <p>
            {project.indexed
              ? "Allocated to tasks"
              : "Observed task allocations"}
          </p>
          <strong>
            <Money value={allocation} unit={false} />
          </strong>
          <small>
            {project.indexed
              ? "USDC · Remaining in task vaults"
              : "USDC · From confirmed networks only"}
          </small>
        </div>
        <div className="stat">
          <p>{project.indexed ? "Open tasks" : "Observed open tasks"}</p>
          <strong>{openTasks.length}</strong>
          <small>
            {project.indexed
              ? "Includes tasks waiting for funds or paused"
              : "Remaining networks are not included yet"}
          </small>
        </div>
      </div>
      <Panel
        title="Funds by network"
        description="Available USDC after existing commitments."
        action={
          <Link to={`${base}/funding`} className="text-link">
            Manage funds ↗
          </Link>
        }
      >
        <div className="network-balances">
          {project.chains.map((chain) => (
            <div className="network-balance" key={chain.chain}>
              <ChainBadge chain={chain.chain} />
              <strong>
                <Money value={chain.available} />
              </strong>
              <span className="review-note">
                {chain.observed_at
                  ? "Available to fund"
                  : "Awaiting network data"}
              </span>
            </div>
          ))}
        </div>
        <details className="disclosure panel-disclosure">
          <summary>Reservations & balance details</summary>
          <div className="disclosure-body">
            <p className="review-note">
              Held funds and pending reservations cannot fund another request.
              These are the latest confirmed balances, so transfers in progress
              may still be awaiting confirmation.
            </p>
            <dl className="definition-list">
              <div>
                <dt>Held for funding</dt>
                <dd>
                  <Money value={project.held} />
                </dd>
              </div>
              <div>
                <dt>Reservations awaiting confirmation</dt>
                <dd>
                  <Money value={project.pending_reserves} />
                </dd>
              </div>
            </dl>
          </div>
          <div
            className="table-scroll"
            tabIndex={0}
            role="region"
            aria-label="Project liquidity across networks"
          >
            <table>
              <thead>
                <tr>
                  <th>Network</th>
                  <th>Available</th>
                  <th>Held</th>
                  <th>Pending reservation</th>
                  <th>Last confirmed</th>
                </tr>
              </thead>
              <tbody>
                {project.chains.map((chain) => (
                  <tr key={chain.chain}>
                    <td>
                      <ChainBadge chain={chain.chain} />
                    </td>
                    <td>
                      <Money value={chain.available} />
                    </td>
                    <td>
                      <Money value={chain.held} />
                    </td>
                    <td>
                      <Money value={chain.pending_reserves} />
                    </td>
                    <td>
                      {chain.observed_at ? (
                        <>
                          <time dateTime={chain.observed_at}>
                            {new Date(chain.observed_at).toLocaleTimeString()}
                          </time>
                          <small>Block / slot {chain.height}</small>
                        </>
                      ) : (
                        "Awaiting network data"
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </details>
      </Panel>
      <div className="split">
        <Panel
          title="Work in progress"
          action={
            <Link to={`${base}/tasks`} className="text-link">
              All tasks →
            </Link>
          }
        >
          {openTasks.length > 0 ? (
            <div className="work-list">
              {openTasks.slice(0, 5).map((task) => {
                const request = project.requests.find(
                  (request) =>
                    request.chain === task.chain && request.task === task.id,
                );
                return (
                  <Link
                    key={`${task.chain}:${task.id}`}
                    to={`${base}/tasks?${new URLSearchParams({ task: task.id, chain: task.chain })}`}
                    className="work-item"
                  >
                    <div className="work-item-main">
                      <strong>
                        {request ? (
                          <ReferenceContent
                            project={project.key}
                            digest={request.purpose}
                            label={`Task ${short(task.id, 5)}`}
                            compact
                          />
                        ) : (
                          `Task ${short(task.id, 5)}`
                        )}
                      </strong>
                      <div className="inline-meta">
                        <ChainBadge chain={task.chain} />
                        <Status value={taskStatus(task, project.requests)} />
                      </div>
                    </div>
                    <div className="work-item-balance">
                      <Money value={task.available} />
                      <small>In task vault</small>
                    </div>
                  </Link>
                );
              })}
            </div>
          ) : (
            <Empty
              title={
                project.tasks.length
                  ? "No open tasks"
                  : "The first task starts here"
              }
              action={
                project.tasks.length > 0 && (
                  <Link className="text-link" to={`${base}/tasks`}>
                    View completed tasks →
                  </Link>
                )
              }
            >
              {project.tasks.length
                ? "Completed and canceled tasks remain available in the task history."
                : "An agent creates a request on its execution network. The manager reviews it before funding."}
            </Empty>
          )}
        </Panel>
        <Panel title="Next steps">
          <div className="panel-body">
            <div className="next-actions">
              {project.roles.authority && awaitingApproval > 0 && (
                <Link to={`${base}/tasks?status=review#task-list`}>
                  <strong>
                    Review budget requests <span aria-hidden="true">→</span>
                  </strong>
                  <span>
                    {awaitingApproval}{" "}
                    {awaitingApproval === 1
                      ? "request awaits"
                      : "requests await"}{" "}
                    your decision. Approval places a request in the funding
                    queue.
                  </span>
                </Link>
              )}
              {project.roles.authority && nextFunding && (
                <Link to={`${base}/tasks`}>
                  <strong>
                    Fund the next approved request{" "}
                    <span aria-hidden="true">→</span>
                  </strong>
                  <span>
                    Review the full amount and delivery costs before signing.
                  </span>
                </Link>
              )}
              {(project.roles.owner ||
                Object.keys(project.roles.local_owners).length > 0) && (
                <Link to={`${base}/funding`}>
                  <strong>
                    Manage project funds <span aria-hidden="true">→</span>
                  </strong>
                  <span>Add USDC or review unused funds.</span>
                </Link>
              )}
              <Link to={`${base}/tasks`}>
                <strong>
                  Follow task progress <span aria-hidden="true">→</span>
                </strong>
                <span>View requests, allocations and available actions.</span>
              </Link>
            </div>
            <details className="disclosure">
              <summary>How funding works</summary>
              <p className="review-note">
                Approval sets the amount and queue position. Funding starts in
                that order. An agent receives its full approved allocation once
                funding is confirmed. The project also pays delivery costs.
              </p>
            </details>
            <Link
              to={`${base}/settings`}
              className="text-link project-settings-link"
            >
              Roles & contract details →
            </Link>
            <p className="context-help">
              <GuideLink
                section={
                  project.roles.authority
                    ? "manager"
                    : project.roles.owner
                      ? "owner"
                      : "agent"
                }
              >
                Step-by-step workspace guide →
              </GuideLink>
            </p>
          </div>
        </Panel>
      </div>
    </>
  );
}

function Settings({
  project,
  act,
}: {
  project: ProjectDetail;
  act: (action: ActionTarget) => void;
}) {
  const route = useLocation();
  const invite = useRef<HTMLDivElement>(null);
  const [copyState, setCopyState] = useState<
    "ready" | "copying" | "copied" | "manual"
  >("ready");
  const projectUrl = `${window.location.origin}/app/projects/${encodeURIComponent(project.key)}`;
  useEffect(() => {
    if (route.hash !== "#invite") return;
    invite.current?.scrollIntoView({ block: "start" });
    invite.current?.focus({ preventScroll: true });
  }, [route.hash]);
  useEffect(() => {
    if (copyState !== "copied") return;
    const timer = window.setTimeout(() => setCopyState("ready"), 2500);
    return () => window.clearTimeout(timer);
  }, [copyState]);
  async function copyProjectLink() {
    setCopyState("copying");
    try {
      await navigator.clipboard.writeText(projectUrl);
      setCopyState("copied");
    } catch {
      setCopyState("manual");
    }
  }
  return (
    <>
      <div id="invite" ref={invite} tabIndex={-1}>
        <Panel
          title="Invite people"
          description="Share this workspace with an agent or someone you want to appoint as manager."
        >
          <div className="panel-body form-stack">
            <div className="invite-link-row">
              <span className="invite-link-value">
                <Link
                  className="text-link"
                  to={`/app/projects/${encodeURIComponent(project.key)}`}
                  title={projectUrl}
                >
                  {project.name}
                </Link>
                <button
                  type="button"
                  className="copy-button"
                  aria-label="Copy project link"
                  title="Copy project link"
                  disabled={copyState === "copying"}
                  onClick={() => void copyProjectLink()}
                >
                  <Icon name={copyState === "copied" ? "check" : "copy"} />
                </button>
                <span className="copy-feedback" role="status">
                  {copyState === "copying"
                    ? "Copying…"
                    : copyState === "copied"
                      ? "Copied"
                      : copyState === "manual"
                        ? "Copy the link below"
                        : ""}
                </span>
              </span>
              <span className="review-note">
                Sharing the link does not grant permissions.
              </span>
            </div>
            {copyState === "manual" && (
              <Field
                label="Project link"
                hint="Clipboard access is unavailable. Select and copy this link manually."
              >
                <input
                  value={projectUrl}
                  readOnly
                  onFocus={(event) => event.currentTarget.select()}
                />
              </Field>
            )}
            <div className="invite-steps">
              <p>
                <strong>For agents:</strong> open the link, sign in and choose
                New task to request a budget. An agent needs manager approval
                and confirmed funding before withdrawing funds.
              </p>
              <p>
                <strong>For a new manager:</strong> sign in, set a username and
                link a NEAR wallet in Account & wallets. The owner can then find
                that @username and choose its verified wallet.
              </p>
            </div>
            <p className="review-note">
              This project has one manager. Appointing someone replaces the
              current manager after the owner reviews and signs the change.
            </p>
            {project.roles.owner ? (
              <div className="button-row">
                <button
                  className="button secondary small"
                  onClick={() => act({ kind: "set_authority" })}
                >
                  <Icon name="manager" />
                  Replace manager
                </button>
              </div>
            ) : (
              <p className="review-note">
                Only the project owner can replace the manager. You can share
                the link with agents without changing project permissions.
              </p>
            )}
          </div>
        </Panel>
      </div>
      <Panel
        title="Project permissions"
        description="Roles are enforced by the contracts. Linking a wallet to a profile does not grant one."
      >
        <div className="panel-body">
          <dl className="definition-list">
            <div>
              <dt>Owner · NEAR</dt>
              <dd>
                <Address value={project.owner} full />
              </dd>
            </div>
            <div>
              <dt>Manager · NEAR</dt>
              <dd>
                <Address value={project.authority} full />
              </dd>
            </div>
            <div>
              <dt>Recovery attempt limit</dt>
              <dd>{project.retry_limit}</dd>
            </div>
          </dl>
          {project.roles.owner && (
            <div className="button-row" style={{ marginTop: 23 }}>
              <button
                className="button secondary small"
                onClick={() => act({ kind: "set_retry_limit" })}
              >
                Change retry limit
              </button>
            </div>
          )}
        </div>
      </Panel>
      <Panel
        title="Contract deployments"
        description="Verify custody addresses and the last indexed block or slot for each network."
      >
        <details className="disclosure deployment-details">
          <summary>Project identifiers</summary>
          <dl className="definition-list disclosure-body">
            <div>
              <dt>Coordinator · NEAR</dt>
              <dd>
                <Address value={project.coordinator} full />
              </dd>
            </div>
            <div>
              <dt>On-chain project ID</dt>
              <dd>{project.project_id}</dd>
            </div>
            <div>
              <dt>Workspace key</dt>
              <dd>
                <Address value={project.key} full />
              </dd>
            </div>
          </dl>
        </details>
        {project.chains.map((chain) => (
          <details className="disclosure deployment-details" key={chain.chain}>
            <summary>
              <ChainBadge chain={chain.chain} />
              <span className="disclosure-meta">
                Addresses & network records
              </span>
            </summary>
            <dl className="definition-list disclosure-body">
              <div>
                <dt>Project vault</dt>
                <dd>
                  <Address value={chain.vault} full />
                </dd>
              </div>
              <div>
                <dt>USDC token</dt>
                <dd>
                  <Address value={chain.asset} full />
                </dd>
              </div>
              <div>
                <dt>Observed block / slot</dt>
                <dd>{chain.height ?? "Waiting"}</dd>
              </div>
              <div>
                <dt>Block reference</dt>
                <dd>
                  {chain.block_hash ? (
                    <Address value={chain.block_hash} full />
                  ) : (
                    "Not available"
                  )}
                </dd>
              </div>
            </dl>
          </details>
        ))}
      </Panel>
    </>
  );
}
