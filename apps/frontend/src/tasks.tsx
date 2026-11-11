import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import type { ProjectDetail, RequestView, TaskView } from "./api";
import type { ActionTarget } from "./action";
import {
  Address,
  ChainBadge,
  Empty,
  Field,
  Money,
  Notice,
  Panel,
  ReferenceContent,
  Status,
  chains,
  chainNames,
  short,
} from "./ui";

export function taskStatus(task: TaskView): string {
  if (task.state !== "active") return task.state;
  if (task.paused) return "paused";
  if (BigInt(task.pending_outflow) > 0n) return "withdrawal pending";
  if (task.started) return "executing";
  return BigInt(task.available) > 0n ? "ready" : "awaiting funding";
}

function currentRequest(task: TaskView | undefined, request: RequestView) {
  return (
    request.local_state === "pending" &&
    task?.live_request === request.id &&
    (task.state === "active" ||
      (task.state === "stopped" && request.kind === "resume"))
  );
}

function pendingTaskControl(
  project: ProjectDetail,
  task: TaskView | undefined,
) {
  return (
    task &&
    project.commands.find(
      (command) =>
        command.chain === task.chain &&
        command.task === task.id &&
        ["pending", "unobserved"].includes(command.status) &&
        [
          "pause",
          "reclaim",
          "continue",
          "cancel",
          "restore_request",
          "out_of_budget",
        ].includes(command.action),
    )
  );
}

export function Tasks({
  project,
  act,
}: {
  project: ProjectDetail;
  act: (action: ActionTarget) => void;
}) {
  const [selected, setSelected] = useSearchParams();
  const expanded = selected.has("task")
    ? `${selected.get("chain")}:${selected.get("task")}`
    : null;
  const [mine, setMine] = useState(false);
  const [network, setNetwork] = useState("all");
  const [status, setStatus] = useState("all");
  const [search, setSearch] = useState("");
  const base = `/app/projects/${encodeURIComponent(project.key)}`;
  function resetFilters() {
    setMine(false);
    setNetwork("all");
    setStatus("all");
    setSearch("");
  }
  const query = search.trim().toLowerCase();
  const tasks = project.tasks.filter((task) => {
    const terminal = ["completed", "canceled"].includes(task.state);
    return (
      (!mine || task.mine) &&
      (network === "all" || task.chain === network) &&
      (status === "all" ||
        (status === "open" && !terminal) ||
        (status === "finished" && terminal) ||
        (status === "paused" && task.paused && !terminal) ||
        (status === "review" &&
          project.requests.some(
            (request) =>
              request.chain === task.chain &&
              request.task === task.id &&
              request.status === "pending" &&
              currentRequest(task, request),
          ))) &&
      (!query ||
        [
          task.id,
          task.agent,
          ...project.requests
            .filter(
              (request) =>
                request.chain === task.chain && request.task === task.id,
            )
            .map((request) => request.id),
        ].some((value) => value.toLowerCase().includes(query)))
    );
  });
  const expandedVisible = tasks.some(
    (task) => `${task.chain}:${task.id}` === expanded,
  );
  useEffect(() => {
    if (expanded && !expandedVisible) resetFilters();
  }, [expanded]);
  const scrollSelection = useRef({ id: expanded, done: false });
  useEffect(() => {
    if (scrollSelection.current.id !== expanded) {
      scrollSelection.current = { id: expanded, done: false };
    }
    if (expanded && expandedVisible && !scrollSelection.current.done) {
      const detail = document.getElementById(`task-detail-${expanded}`);
      if (detail) {
        detail.scrollIntoView({ block: "nearest" });
        scrollSelection.current.done = true;
      }
    }
  }, [expanded, expandedVisible]);
  const queue = project.requests
    .filter(
      (request) =>
        request.approval_position !== null &&
        !["funded", "removed", "rejected"].includes(request.status),
    )
    .sort((a, b) => (a.approval_position ?? 0) - (b.approval_position ?? 0));
  return (
    <>
      <Panel
        title="Approval-order queue"
        description="A later request cannot bypass the first waiting request, even if its network already holds funds."
      >
        {queue.length ? (
          <div
            className="table-scroll"
            tabIndex={0}
            role="region"
            aria-label="Approval-order queue"
          >
            <table>
              <thead>
                <tr>
                  <th>Priority</th>
                  <th>Network / request</th>
                  <th>Approved amount</th>
                  <th>Funding state</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {queue.map((request) => {
                  const task = project.tasks.find(
                    (task) =>
                      task.chain === request.chain && task.id === request.task,
                  );
                  const current = currentRequest(task, request);
                  const control = pendingTaskControl(project, task);
                  return (
                    <tr key={`${request.chain}:${request.id}`}>
                      <td>
                        {request.queue_head ? (
                          <span className="role-badge">Next in line</span>
                        ) : (
                          `#${(request.approval_position ?? 0) + 1}`
                        )}
                      </td>
                      <td>
                        <ChainBadge chain={request.chain} />
                        <small>
                          <Link
                            className="text-link"
                            to={`${base}/tasks?${new URLSearchParams({ chain: request.chain, task: request.task })}`}
                          >
                            Task {short(request.task, 6)}
                          </Link>
                        </small>
                        <small>
                          Request <Address value={request.id} />
                        </small>
                      </td>
                      <td>
                        <Money value={request.amount} />
                      </td>
                      <td>
                        <Status value={request.status} />
                      </td>
                      <td>
                        {request.queue_head &&
                          request.status === "queued" &&
                          project.roles.authority &&
                          !request.plan &&
                          current &&
                          !control && (
                            <button
                              className="button small"
                              onClick={() => act({ kind: "apply", request })}
                            >
                              Review funding
                            </button>
                          )}
                        {request.plan && (
                          <Link
                            className="text-link"
                            to={`${base}/activity?${new URLSearchParams({ plan: request.plan })}`}
                          >
                            View funding plan →
                          </Link>
                        )}
                        {!request.plan && !current && (
                          <small>
                            Waiting for the coordinator to synchronize the
                            task’s local state.
                          </small>
                        )}
                        {!request.plan && current && control && (
                          <small>
                            Waiting for the task’s project control to settle.{" "}
                            <Link className="text-link" to={`${base}/activity`}>
                              View activity →
                            </Link>
                          </small>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ) : (
          <Empty title="No requests waiting in the queue">
            Approvals join this queue in the order they execute on NEAR.
          </Empty>
        )}
      </Panel>
      <Panel
        title="Tasks"
        description="Requests originate on the network where the agent will work."
        action={
          <button
            className="button small secondary"
            aria-pressed={mine}
            onClick={() => setMine(!mine)}
          >
            {mine ? "Show all tasks" : "My tasks"}
          </button>
        }
      >
        <div className="list-filters">
          <Field
            label="Find a task"
            hint="Search by task ID, request ID or agent address."
          >
            <input
              type="search"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Task, request or agent"
            />
          </Field>
          <Field label="Task network">
            <select
              value={network}
              onChange={(event) => setNetwork(event.target.value)}
            >
              <option value="all">All networks</option>
              {chains.map((chain) => (
                <option key={chain} value={chain}>
                  {chainNames[chain]}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Task status">
            <select
              value={status}
              onChange={(event) => setStatus(event.target.value)}
            >
              <option value="all">All tasks</option>
              <option value="review">Awaiting approval</option>
              <option value="open">Open tasks</option>
              <option value="paused">Owner paused</option>
              <option value="finished">Completed or canceled</option>
            </select>
          </Field>
        </div>
        <p className="filter-summary" role="status">
          {tasks.length} of {project.tasks.length} tasks
        </p>
        {expanded &&
          !project.tasks.some(
            (task) => `${task.chain}:${task.id}` === expanded,
          ) && (
            <Notice>
              The linked task is not present in the latest project observation.
              Check its network and identity, or refresh after it is finalized.
            </Notice>
          )}
        {tasks.length ? (
          <div>
            {tasks.map((task) => {
              const key = `${task.chain}:${task.id}`;
              return (
                <TaskCard
                  key={key}
                  project={project}
                  task={task}
                  requests={project.requests.filter(
                    (request) =>
                      request.chain === task.chain && request.task === task.id,
                  )}
                  expanded={expanded === key}
                  act={act}
                  onToggle={() => {
                    const next = new URLSearchParams(selected);
                    if (expanded === key) {
                      next.delete("task");
                      next.delete("chain");
                    } else {
                      next.set("task", task.id);
                      next.set("chain", task.chain);
                    }
                    setSelected(next);
                  }}
                />
              );
            })}
          </div>
        ) : (
          <Empty
            title={
              project.tasks.length
                ? "No tasks match these filters"
                : "No tasks yet"
            }
            action={
              project.tasks.length ? (
                <button
                  className="button secondary small"
                  onClick={resetFilters}
                >
                  Clear filters
                </button>
              ) : (
                <button
                  className="button secondary small"
                  onClick={() => act({ kind: "create_task" })}
                >
                  Create a task
                </button>
              )
            }
          >
            {project.tasks.length
              ? "Clear the filters or search using another task, request or agent identity."
              : "Create a task on its execution network to request a budget."}
          </Empty>
        )}
      </Panel>
    </>
  );
}

function TaskCard({
  project,
  task,
  requests,
  expanded,
  onToggle,
  act,
}: {
  project: ProjectDetail;
  task: TaskView;
  requests: RequestView[];
  expanded: boolean;
  onToggle: () => void;
  act: (action: ActionTarget) => void;
}) {
  const key = `${task.chain}:${task.id}`;
  const base = `/app/projects/${encodeURIComponent(project.key)}`;
  const terminal = ["completed", "canceled"].includes(task.state);
  const control = pendingTaskControl(project, task);
  const available = BigInt(task.available) > 0n;
  const external =
    BigInt(task.issued) -
    BigInt(task.recovered) -
    BigInt(task.available) -
    BigInt(task.pending_outflow);
  return (
    <div key={key}>
      <div className="operation-card">
        <div className="operation-title">
          <div>
            <button
              className="expand-button"
              aria-expanded={expanded}
              aria-controls={`task-detail-${key}`}
              onClick={onToggle}
            >
              {requests[0] ? (
                <ReferenceContent
                  project={project.key}
                  digest={requests[0].purpose}
                  label={`Task ${short(task.id, 6)}`}
                  compact
                />
              ) : (
                `Task ${short(task.id, 6)}`
              )}{" "}
              {expanded ? "−" : "+"}
            </button>
            <p style={{ marginTop: 7 }}>
              <ChainBadge chain={task.chain} />{" "}
              <span style={{ marginLeft: 12 }}>
                Agent <Address value={task.agent} />
                {task.mine && " · You"}
              </span>
            </p>
          </div>
          <Status value={taskStatus(task)} />
        </div>
        <div className="inline-meta">
          <span>
            Available <Money value={task.available} />
          </span>
          <span>
            Issued <Money value={task.issued} />
          </span>
          {task.paused && <span>Owner pause active</span>}
        </div>
      </div>
      {expanded && (
        <div className="task-detail" id={`task-detail-${key}`}>
          <div className="stats">
            <div className="stat">
              <p>Available allocation</p>
              <strong>
                <Money value={task.available} />
              </strong>
            </div>
            <div className="stat">
              <p>Total issued</p>
              <strong>
                <Money value={task.issued} />
              </strong>
            </div>
            <div className="stat">
              <p>Recovered by project</p>
              <strong>
                <Money value={task.recovered} />
              </strong>
            </div>
            <div className="stat">
              <p>Pending withdrawal</p>
              <strong>
                <Money value={task.pending_outflow} />
              </strong>
            </div>
          </div>
          {task.paused && !terminal && (
            <Notice>
              Owner pause blocks new withdrawals. Returning and releasing funds
              remain available.
            </Notice>
          )}
          {BigInt(task.pending_outflow) > 0n && (
            <Notice>
              A withdrawal is still being settled. Its amount remains
              unavailable until the local outcome is confirmed.
            </Notice>
          )}
          {task.state === "stopped" && (
            <Notice>
              This task stopped after an additional budget request was rejected.
              The agent can request a new budget to resume through approval and
              funding. An owner pause remains separate.
            </Notice>
          )}
          {task.state === "out_of_budget" && (
            <Notice>
              This task cannot receive another allocation. If it was funded,
              return wallet leftovers and release its remaining allocation
              before completion. Create a new task for further work.
            </Notice>
          )}
          {task.mine && !terminal && (
            <div className="row-actions">
              {task.state === "active" && !task.paused && available && (
                <button
                  className="button small"
                  onClick={() => act({ kind: "withdraw", task })}
                >
                  Withdraw
                </button>
              )}
              {!task.live_request &&
                (task.state === "stopped" ||
                  (task.state === "active" &&
                    (BigInt(task.issued) === 0n || task.started))) && (
                  <button
                    className="button secondary small"
                    onClick={() => act({ kind: "request_budget", task })}
                  >
                    Request budget
                  </button>
                )}
              {external > 0n && (
                <button
                  className="button secondary small"
                  onClick={() => act({ kind: "return_funds", task })}
                >
                  Return wallet funds
                </button>
              )}
              {available && (
                <button
                  className="button secondary small"
                  onClick={() => act({ kind: "release", task })}
                >
                  Release allocation
                </button>
              )}
              {BigInt(task.issued) > 0n &&
                !available &&
                BigInt(task.pending_outflow) === 0n && (
                  <button
                    className="button secondary small"
                    onClick={() => act({ kind: "finalize", task })}
                  >
                    Complete task
                  </button>
                )}
            </div>
          )}
          {!terminal && control && (
            <Notice>
              A project control is awaiting chain confirmation. Further project
              controls and funding approvals wait for it to settle.{" "}
              <Link className="text-link" to={`${base}/activity`}>
                View activity →
              </Link>
            </Notice>
          )}
          {!terminal &&
            !control &&
            (project.roles.owner || project.roles.authority) && (
              <>
                <p className="section-label">Project controls</p>
                <div className="row-actions">
                  {project.roles.owner && (
                    <>
                      {!task.started && BigInt(task.pending_outflow) === 0n && (
                        <button
                          className="button secondary small"
                          onClick={() => act({ kind: "cancel", task })}
                        >
                          Cancel before use
                        </button>
                      )}
                      {!task.paused && (
                        <button
                          className="button secondary small"
                          onClick={() => act({ kind: "pause", task })}
                        >
                          Pause withdrawals
                        </button>
                      )}
                      {task.paused && available && (
                        <button
                          className="button secondary small"
                          onClick={() => act({ kind: "reclaim", task })}
                        >
                          Reclaim allocation
                        </button>
                      )}
                      {task.paused &&
                        available &&
                        task.state === "active" &&
                        BigInt(task.pending_outflow) === 0n && (
                          <button
                            className="button secondary small"
                            onClick={() => act({ kind: "continue", task })}
                          >
                            Continue task
                          </button>
                        )}
                    </>
                  )}
                  {project.roles.authority && (
                    <>
                      {task.state !== "out_of_budget" && (
                        <button
                          className="button secondary small"
                          onClick={() => act({ kind: "out_of_budget", task })}
                        >
                          Mark out of budget
                        </button>
                      )}
                      {task.paused &&
                        task.state === "active" &&
                        BigInt(task.issued) > 0n &&
                        !task.live_request && (
                          <button
                            className="button secondary small"
                            onClick={() => act({ kind: "restore", task })}
                          >
                            Restore allocation
                          </button>
                        )}
                    </>
                  )}
                </div>
              </>
            )}
          <p className="section-label">Budget requests</p>
          {!requests.length && (
            <p className="review-note">
              The task’s requests are still being indexed. Refresh after the
              local transaction is finalized.
            </p>
          )}
          {requests.map((request) => (
            <div
              className="review"
              style={{ marginBottom: 12 }}
              key={request.id}
            >
              <div className="operation-title">
                <div>
                  <strong>
                    <Money value={request.amount} />
                  </strong>
                  <p style={{ marginTop: 5 }}>
                    {request.kind} request · {short(request.id, 6)}
                  </p>
                </div>
                <Status value={request.status} />
              </div>
              <ReferenceContent
                project={project.key}
                digest={request.purpose}
                label="Purpose"
              />
              {request.plan && (
                <Link
                  className="text-link"
                  to={`${base}/activity?${new URLSearchParams({ plan: request.plan })}`}
                >
                  View funding plan →
                </Link>
              )}
              {project.roles.authority &&
                ["pending", "queued"].includes(request.status) &&
                currentRequest(task, request) &&
                !request.plan && (
                  <div className="button-row" style={{ marginTop: 15 }}>
                    {request.status === "pending" && !control && (
                      <button
                        className="button small"
                        onClick={() => act({ kind: "approve", request })}
                      >
                        Approve
                      </button>
                    )}
                    <button
                      className="button small secondary"
                      onClick={() => act({ kind: "reject", request })}
                    >
                      Reject
                    </button>
                  </div>
                )}
              {["pending", "queued"].includes(request.status) &&
                !currentRequest(task, request) && (
                  <p className="review-note" style={{ marginTop: 12 }}>
                    The coordinator is still synchronizing this request’s local
                    state. It is no longer available for approval or funding in
                    the latest local observation.
                  </p>
                )}
            </div>
          ))}
          {task.report && (
            <ReferenceContent
              project={project.key}
              digest={task.report}
              label="Completion report"
            />
          )}
          <details>
            <summary>Task identity and accounting</summary>
            <p className="review-note" style={{ marginTop: 12 }}>
              Task ID: <code>{task.id}</code>
              <br />
              Agent: <code>{task.agent}</code>
              <br />
              {task.state === "completed"
                ? `Recorded expense: `
                : "Issued minus recovered: "}
              <Money
                value={(
                  BigInt(task.issued) - BigInt(task.recovered)
                ).toString()}
              />
              . This accounting does not prove particular purchases or an empty
              external wallet.
            </p>
          </details>
        </div>
      )}
    </div>
  );
}
