import { useState } from "react";
import { useLocation } from "react-router-dom";
import type { ProjectDetail, TaskView } from "./api";
import type { ActionTarget } from "./action";
import {
  Address,
  ChainBadge,
  Empty,
  Money,
  Notice,
  Panel,
  ReferenceContent,
  Status,
  short,
} from "./ui";

export function taskStatus(task: TaskView): string {
  if (task.state !== "active") return task.state;
  if (task.paused) return "paused";
  if (BigInt(task.pending_outflow) > 0n) return "withdrawal pending";
  if (task.started) return "executing";
  return BigInt(task.available) > 0n ? "ready" : "awaiting funding";
}

export function Tasks({
  project,
  act,
}: {
  project: ProjectDetail;
  act: (action: ActionTarget) => void;
}) {
  const location = useLocation();
  const selected = new URLSearchParams(location.search);
  const [expanded, setExpanded] = useState<string | null>(
    selected.has("task")
      ? `${selected.get("chain")}:${selected.get("task")}`
      : null,
  );
  const [mine, setMine] = useState(false);
  const tasks = project.tasks.filter((task) => !mine || task.mine);
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
            aria-label="Scrollable project table"
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
                {queue.map((request) => (
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
                        <Address value={request.id} />
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
                        project.roles.authority &&
                        !request.plan && (
                          <button
                            className="button small"
                            onClick={() => act({ kind: "apply", request })}
                          >
                            Review funding
                          </button>
                        )}
                    </td>
                  </tr>
                ))}
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
        {tasks.length ? (
          <div>
            {tasks.map((task) => {
              const key = `${task.chain}:${task.id}`;
              const requests = project.requests.filter(
                (request) =>
                  request.chain === task.chain && request.task === task.id,
              );
              const terminal = ["completed", "canceled"].includes(task.state);
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
                          aria-expanded={expanded === key}
                          onClick={() =>
                            setExpanded(expanded === key ? null : key)
                          }
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
                          {expanded === key ? "−" : "+"}
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
                  {expanded === key && (
                    <div className="task-detail">
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
                      {task.paused && (
                        <Notice>
                          Owner pause blocks new withdrawals. Returning and
                          releasing funds remain available.
                        </Notice>
                      )}
                      {task.mine && !terminal && (
                        <div className="row-actions">
                          {task.state === "active" &&
                            !task.paused &&
                            available && (
                              <button
                                className="button small"
                                onClick={() => act({ kind: "withdraw", task })}
                              >
                                Withdraw
                              </button>
                            )}
                          {!task.live_request &&
                            task.state !== "out_of_budget" && (
                              <button
                                className="button secondary small"
                                onClick={() =>
                                  act({ kind: "request_budget", task })
                                }
                              >
                                Request budget
                              </button>
                            )}
                          {external > 0n && (
                            <button
                              className="button secondary small"
                              onClick={() =>
                                act({ kind: "return_funds", task })
                              }
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
                      {!terminal &&
                        (project.roles.owner || project.roles.authority) && (
                          <>
                            <p className="section-label">Project controls</p>
                            <div className="row-actions">
                              {project.roles.owner && (
                                <>
                                  {!task.started && (
                                    <button
                                      className="button secondary small"
                                      onClick={() =>
                                        act({ kind: "cancel", task })
                                      }
                                    >
                                      Cancel before use
                                    </button>
                                  )}
                                  {!task.paused && (
                                    <button
                                      className="button secondary small"
                                      onClick={() =>
                                        act({ kind: "pause", task })
                                      }
                                    >
                                      Pause withdrawals
                                    </button>
                                  )}
                                  {task.paused && available && (
                                    <button
                                      className="button secondary small"
                                      onClick={() =>
                                        act({ kind: "reclaim", task })
                                      }
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
                                        onClick={() =>
                                          act({ kind: "continue", task })
                                        }
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
                                      onClick={() =>
                                        act({ kind: "out_of_budget", task })
                                      }
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
                                        onClick={() =>
                                          act({ kind: "restore", task })
                                        }
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
                          {project.roles.authority &&
                            ["pending", "queued"].includes(request.status) &&
                            !request.plan && (
                              <div
                                className="button-row"
                                style={{ marginTop: 15 }}
                              >
                                {request.status === "pending" && (
                                  <button
                                    className="button small"
                                    onClick={() =>
                                      act({ kind: "approve", request })
                                    }
                                  >
                                    Approve
                                  </button>
                                )}
                                <button
                                  className="button small secondary"
                                  onClick={() =>
                                    act({ kind: "reject", request })
                                  }
                                >
                                  Reject
                                </button>
                              </div>
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
                          . This accounting does not prove particular purchases
                          or an empty external wallet.
                        </p>
                      </details>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        ) : (
          <Empty
            title={mine ? "No tasks for your linked wallets" : "No tasks yet"}
            action={
              <button
                className="button secondary small"
                onClick={() => act({ kind: "create_task" })}
              >
                Create a task
              </button>
            }
          >
            Create a task on its execution network to request a budget.
          </Empty>
        )}
      </Panel>
    </>
  );
}
