import { useEffect, useState } from "react";
import { Link, NavLink, useLocation, useParams } from "react-router-dom";
import { api, type ProjectDetail } from "./api";
import { ActionDialog, type ActionTarget } from "./action";
import { RoleBadges } from "./pages";
import { Tasks, taskStatus } from "./tasks";
import { Funding } from "./funding";
import { Activity } from "./activity";
import {
  Address,
  ChainBadge,
  Empty,
  ErrorState,
  Loading,
  Money,
  Notice,
  PageTitle,
  Panel,
  Status,
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
        title={short(project.coordinator, 20)}
        action={
          <div className="button-row">
            <button
              className="button secondary small"
              disabled={result.loading}
              onClick={result.refresh}
            >
              Refresh
            </button>
            <button
              className="button small"
              onClick={() => setAction({ kind: "create_task" })}
            >
              New task +
            </button>
          </div>
        }
      >
        <span className="inline-meta">
          <RoleBadges roles={project.roles} />
          <span>
            Updated{" "}
            {project.observed_at
              ? new Date(project.observed_at).toLocaleString()
              : "not yet observed"}
          </span>
        </span>
      </PageTitle>
      {result.error !== undefined && (
        <Notice tone="error">
          Refresh failed. Displaying the last observed state; available balances
          may have changed.{" "}
          {result.error instanceof Error ? result.error.message : ""}
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
      {tab === "tasks" && <Tasks project={project} act={setAction} />}
      {tab === "funding" && <Funding project={project} act={setAction} />}
      {tab === "activity" && <Activity project={project} act={setAction} />}
      {tab === "settings" && <Settings project={project} act={setAction} />}
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

function Overview({ project, base }: { project: ProjectDetail; base: string }) {
  const allocation = project.tasks
    .reduce((sum, task) => sum + BigInt(task.available), 0n)
    .toString();
  const attention = project.plans.filter((plan) => plan.requires_attention);
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
      <div className="stats">
        <div className="stat">
          <p>Eligible for funding</p>
          <strong>
            <Money value={project.available} unit={false} />
          </strong>
          <small>USDC · Free after pending reservations</small>
        </div>
        <div className="stat">
          <p>Held in funding plans</p>
          <strong>
            <Money value={project.held} unit={false} />
          </strong>
          <small>USDC · Already occupied</small>
        </div>
        <div className="stat">
          <p>Reservations pending</p>
          <strong>
            <Money value={project.pending_reserves} unit={false} />
          </strong>
          <small>USDC · Excluded from eligible funds</small>
        </div>
        <div className="stat">
          <p>Agent allocations</p>
          <strong>
            <Money value={allocation} unit={false} />
          </strong>
          <small>USDC · Still controlled by local vaults</small>
        </div>
      </div>
      <Panel
        title="Liquidity across networks"
        description="Coordinator balances are confirmed observations. Pending operations remain unavailable."
        action={
          <Link to={`${base}/funding`} className="text-link">
            Manage funds ↗
          </Link>
        }
      >
        <div
          className="table-scroll"
          tabIndex={0}
          role="region"
          aria-label="Scrollable project table"
        >
          <table>
            <thead>
              <tr>
                <th>Network</th>
                <th>Eligible funds</th>
                <th>Held</th>
                <th>Pending reservation</th>
                <th>Local observation</th>
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
                      "Waiting for indexer"
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
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
          {project.tasks.some(
            (task) => !["completed", "canceled"].includes(task.state),
          ) ? (
            <div
              className="table-scroll"
              tabIndex={0}
              role="region"
              aria-label="Scrollable project table"
            >
              <table>
                <thead>
                  <tr>
                    <th>Task</th>
                    <th>Network</th>
                    <th>State</th>
                    <th>Available</th>
                  </tr>
                </thead>
                <tbody>
                  {project.tasks
                    .filter(
                      (task) => !["completed", "canceled"].includes(task.state),
                    )
                    .slice(0, 5)
                    .map((task) => (
                      <tr key={`${task.chain}:${task.id}`}>
                        <td>
                          <Link
                            to={`${base}/tasks?task=${task.id}&chain=${task.chain}`}
                            className="text-link"
                          >
                            {short(task.id, 5)}
                          </Link>
                        </td>
                        <td>
                          <ChainBadge chain={task.chain} />
                        </td>
                        <td>
                          <Status value={taskStatus(task)} />
                        </td>
                        <td>
                          <Money value={task.available} />
                        </td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </div>
          ) : (
            <Empty title="The first task starts here">
              An agent creates a request on its execution network. The manager
              reviews it before funding.
            </Empty>
          )}
        </Panel>
        <Panel title="Your project controls">
          <div className="panel-body">
            <dl className="definition-list">
              <div>
                <dt>Owner</dt>
                <dd>
                  <Address value={project.owner} />
                </dd>
              </div>
              <div>
                <dt>Manager</dt>
                <dd>
                  <Address value={project.authority} />
                </dd>
              </div>
              <div>
                <dt>Coordination</dt>
                <dd>NEAR</dd>
              </div>
              <div>
                <dt>Budget asset</dt>
                <dd>USDC · 6 decimals</dd>
              </div>
            </dl>
            <p className="review-note" style={{ marginTop: 17 }}>
              Approval establishes priority. Funding happens in that order, for
              the full amount. Delivery fees are paid separately from the
              project budget.
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
  return (
    <>
      <Panel
        title="Project permissions"
        description="Roles are enforced by the contracts. Linking a wallet to a profile does not grant one."
      >
        <div className="panel-body">
          <dl className="definition-list">
            <div>
              <dt>Owner · NEAR</dt>
              <dd>{project.owner}</dd>
            </div>
            <div>
              <dt>Manager · NEAR authority</dt>
              <dd>{project.authority}</dd>
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
                onClick={() => act({ kind: "set_authority" })}
              >
                Replace manager
              </button>
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
        {project.chains.map((chain) => (
          <div className="operation-card" key={chain.chain}>
            <ChainBadge chain={chain.chain} />
            <dl className="definition-list" style={{ marginTop: 12 }}>
              <div>
                <dt>Project vault</dt>
                <dd>
                  <code>{chain.vault}</code>
                </dd>
              </div>
              <div>
                <dt>USDC token</dt>
                <dd>
                  <code>{chain.asset}</code>
                </dd>
              </div>
              <div>
                <dt>Observed block / slot</dt>
                <dd>{chain.height ?? "Waiting"}</dd>
              </div>
              <div>
                <dt>Block reference</dt>
                <dd>
                  <code>{chain.block_hash ?? "Not available"}</code>
                </dd>
              </div>
            </dl>
          </div>
        ))}
      </Panel>
    </>
  );
}
