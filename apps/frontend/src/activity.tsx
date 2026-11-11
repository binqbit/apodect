import { useState } from "react";
import { api, type ProjectDetail } from "./api";
import type { ActionTarget } from "./action";
import {
  Address,
  ChainBadge,
  Empty,
  Loading,
  Money,
  Notice,
  Panel,
  Status,
  chainNames,
  short,
} from "./ui";

export function Activity({
  project,
  act,
}: {
  project: ProjectDetail;
  act: (action: ActionTarget) => void;
}) {
  const [diagnostic, setDiagnostic] = useState<{
    route: string;
    value?: unknown;
    error?: string;
  } | null>(null);
  async function inspect(route: string) {
    setDiagnostic({ route });
    try {
      setDiagnostic({
        route,
        value: await api.transportStatus(project.key, route),
      });
    } catch (failure) {
      setDiagnostic({
        route,
        error:
          failure instanceof Error
            ? failure.message
            : "Provider status unavailable.",
      });
    }
  }
  return (
    <>
      <Panel
        title="Funding plans"
        description="Reservation, source execution, destination receipt and final allocation are separate milestones."
      >
        {project.plans.length ? (
          [...project.plans].reverse().map((plan) => (
            <div key={plan.id} className="operation-card">
              <div className="operation-title">
                <div>
                  <h3>Plan {short(plan.id, 6)}</h3>
                  <p style={{ marginTop: 7 }}>
                    <Money value={plan.principal} /> to{" "}
                    {chainNames[plan.destination]} · Attempt {plan.attempts}
                  </p>
                </div>
                <Status
                  value={plan.requires_attention ? "attention" : plan.status}
                />
              </div>
              {plan.error && <Notice tone="error">{plan.error}</Notice>}
              <div className="inline-meta">
                <span>
                  Local portion <Money value={plan.local_amount} />
                </span>
                <span>
                  Expires{" "}
                  {new Date(Number(plan.expires) * 1000).toLocaleString()}
                </span>
              </div>
              {plan.routes.length > 0 && (
                <div
                  className="table-scroll"
                  tabIndex={0}
                  role="region"
                  aria-label="Scrollable project table"
                  style={{ marginTop: 17 }}
                >
                  <table>
                    <thead>
                      <tr>
                        <th>Route</th>
                        <th>Source debit</th>
                        <th>Destination receipt</th>
                        <th>Refund</th>
                        <th>Unsettled source amount</th>
                        <th>Transport expense</th>
                        <th />
                      </tr>
                    </thead>
                    <tbody>
                      {plan.routes.map((route) => (
                        <tr key={route.id}>
                          <td>
                            <ChainBadge chain={route.source} />
                            <small>→ {chainNames[plan.destination]}</small>
                          </td>
                          <td>
                            <Money value={route.dispatched} />
                          </td>
                          <td>
                            <Money value={route.received} />
                          </td>
                          <td>
                            <Money value={route.returned} />
                          </td>
                          <td>
                            <Money value={route.unsettled_input} />
                          </td>
                          <td>
                            {route.confirmed_transport_expense === null ? (
                              "Not settled"
                            ) : (
                              <Money
                                value={route.confirmed_transport_expense}
                              />
                            )}
                          </td>
                          <td>
                            <button
                              className="subtle-button"
                              onClick={() => void inspect(route.id)}
                            >
                              Provider details
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {plan.routes.length === 0 && (
                <p className="review-note" style={{ marginTop: 15 }}>
                  Local allocation; this plan does not require a transport
                  route.
                </p>
              )}
              {diagnostic &&
                plan.routes.some((route) => route.id === diagnostic.route) && (
                  <div style={{ marginTop: 16 }}>
                    <Notice>
                      Provider status is diagnostic. Only verified chain
                      receipts change the project’s accounting.
                    </Notice>
                    {diagnostic.error ? (
                      <Notice tone="error">{diagnostic.error}</Notice>
                    ) : diagnostic.value === undefined ? (
                      <Loading>Fetching provider status…</Loading>
                    ) : (
                      <pre>{JSON.stringify(diagnostic.value, null, 2)}</pre>
                    )}
                  </div>
                )}
              {(project.roles.authority || project.roles.owner) &&
                plan.status !== "funded" && (
                  <details>
                    <summary>Plan recovery</summary>
                    <p className="review-note" style={{ marginTop: 12 }}>
                      Recovery retains already submitted transfers and their
                      receipts. It never treats an unknown result as a failed
                      payment.
                    </p>
                    <div className="operation-actions">
                      {plan.status !== "closed" && (
                        <button
                          className="button secondary small"
                          onClick={() => act({ kind: "abort", plan })}
                        >
                          Stop and settle plan
                        </button>
                      )}
                      {project.roles.authority && plan.status !== "closed" && (
                        <>
                          <button
                            className="button secondary small"
                            onClick={() => act({ kind: "retry", plan })}
                          >
                            Retry verification
                          </button>
                          <button
                            className="button secondary small"
                            onClick={() => act({ kind: "resume_plan", plan })}
                          >
                            Resume plan
                          </button>
                        </>
                      )}
                      {project.roles.authority &&
                        plan.status === "closed" &&
                        project.requests.some(
                          (request) =>
                            request.plan === plan.id &&
                            request.local_state === "pending",
                        ) &&
                        plan.routes.every(
                          (route) =>
                            BigInt(route.dispatched) === 0n ||
                            BigInt(route.returned) > 0n,
                        ) && (
                          <button
                            className="button secondary small"
                            onClick={() => act({ kind: "requeue", plan })}
                          >
                            Requeue settled request
                          </button>
                        )}
                    </div>
                  </details>
                )}
            </div>
          ))
        ) : (
          <Empty title="No funding plans yet">
            The manager starts a plan from the head of the approval queue.
          </Empty>
        )}
      </Panel>
      {project.commands.length > 0 && (
        <Panel
          title="Coordinator commands"
          description="Commands authorize a specific destination action. Their result is checked independently of submission."
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
                  <th>Command</th>
                  <th>Network</th>
                  <th>State</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {project.commands.map((command) => (
                  <tr key={command.id}>
                    <td>
                      {command.action.replaceAll("_", " ")}
                      <small>
                        <Address value={command.id} />
                      </small>
                      {command.signature_error && (
                        <small>{command.signature_error}</small>
                      )}
                    </td>
                    <td>
                      <ChainBadge chain={command.chain} />
                    </td>
                    <td>
                      <Status value={command.status} />
                    </td>
                    <td>
                      {(project.roles.owner || project.roles.authority) &&
                        ["pending", "unobserved"].includes(command.status) && (
                          <button
                            className="button secondary small"
                            onClick={() => act({ kind: "invalidate", command })}
                          >
                            Invalidate command
                          </button>
                        )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Panel>
      )}
      <Panel
        title="Chain operations"
        description="Submitted and uncertain operations retain their transaction identity until their outcome is known."
      >
        {project.operations.length ? (
          <div
            className="table-scroll"
            tabIndex={0}
            role="region"
            aria-label="Scrollable project table"
          >
            <table>
              <thead>
                <tr>
                  <th>Operation</th>
                  <th>Network</th>
                  <th>State</th>
                  <th>Transaction</th>
                </tr>
              </thead>
              <tbody>
                {project.operations.map((operation) => (
                  <tr key={operation.id}>
                    <td>
                      <Address value={operation.id} />
                      <small>Attempt {operation.attempt}</small>
                      {operation.error && <small>{operation.error}</small>}
                    </td>
                    <td>
                      <ChainBadge chain={operation.chain} />
                    </td>
                    <td>
                      <Status value={operation.state} />
                    </td>
                    <td>
                      {operation.transaction_id ? (
                        <Address value={operation.transaction_id} />
                      ) : (
                        "Not submitted"
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <Empty title="No chain operations recorded">
            Worker activity appears here as contracts progress.
          </Empty>
        )}
      </Panel>
    </>
  );
}
