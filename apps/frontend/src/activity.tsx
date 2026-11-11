import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api, type PlanView, type ProjectDetail } from "./api";
import type { ActionTarget } from "./action";
import {
  Address,
  ChainBadge,
  Empty,
  Field,
  Loading,
  Money,
  Notice,
  Panel,
  Status,
  chains,
  chainNames,
  short,
} from "./ui";

interface TransportDiagnostic {
  route: string;
  value?: unknown;
  error?: string;
}

export function Activity({
  project,
  act,
}: {
  project: ProjectDetail;
  act: (action: ActionTarget) => void;
}) {
  const [selected, setSelected] = useSearchParams();
  const selectedPlan = selected.get("plan");
  const [network, setNetwork] = useState("all");
  const [planStatus, setPlanStatus] = useState("all");
  const [operationStatus, setOperationStatus] = useState("all");
  const [search, setSearch] = useState("");
  const base = `/app/projects/${encodeURIComponent(project.key)}`;
  const query = search.trim().toLowerCase();
  const matchesSearch = (...values: (string | null)[]) =>
    !query || values.some((value) => value?.toLowerCase().includes(query));
  const plans = [...project.plans].reverse().filter((plan) => {
    const terminal = ["funded", "closed"].includes(plan.status);
    const request = project.requests.find(
      (request) => request.plan === plan.id,
    );
    return (
      (!selectedPlan || plan.id === selectedPlan) &&
      (network === "all" ||
        plan.destination === network ||
        plan.routes.some((route) => route.source === network)) &&
      (planStatus === "all" ||
        (planStatus === "open" && !terminal) ||
        (planStatus === "settled" && terminal) ||
        (planStatus === "attention" && plan.requires_attention && !terminal)) &&
      matchesSearch(
        plan.id,
        request?.id ?? null,
        request?.task ?? null,
        ...plan.routes.map((route) => route.id),
      )
    );
  });
  const selectedPlanVisible = plans.some((plan) => plan.id === selectedPlan);
  const scrollSelection = useRef({ id: selectedPlan, done: false });
  useEffect(() => {
    if (scrollSelection.current.id !== selectedPlan) {
      scrollSelection.current = { id: selectedPlan, done: false };
    }
    if (selectedPlan && selectedPlanVisible && !scrollSelection.current.done) {
      const plan = document.getElementById(`funding-plan-${selectedPlan}`);
      if (plan) {
        plan.scrollIntoView({ block: "nearest" });
        scrollSelection.current.done = true;
      }
    }
  }, [selectedPlan, selectedPlanVisible]);
  const commands = project.commands.filter(
    (command) =>
      (network === "all" || command.chain === network) &&
      (operationStatus === "all" ||
        (operationStatus === "open" &&
          ["pending", "unobserved"].includes(command.status)) ||
        (operationStatus === "attention" &&
          (command.status === "failed" ||
            (["pending", "unobserved"].includes(command.status) &&
              Boolean(command.signature_error)))) ||
        (operationStatus === "settled" &&
          ["succeeded", "invalidated"].includes(command.status))) &&
      matchesSearch(command.id, command.plan, command.action),
  );
  const operations = project.operations.filter(
    (operation) =>
      (network === "all" || operation.chain === network) &&
      (operationStatus === "all" ||
        (operationStatus === "open" &&
          ["planned", "prepared", "submitted"].includes(operation.state)) ||
        (operationStatus === "attention" &&
          ["failed", "attention"].includes(operation.state)) ||
        (operationStatus === "settled" && operation.state === "confirmed")) &&
      matchesSearch(operation.id, operation.transaction_id),
  );
  const executionIssues =
    commands.filter(
      (command) =>
        command.status === "failed" ||
        (["pending", "unobserved"].includes(command.status) &&
          Boolean(command.signature_error)),
    ).length +
    operations.filter((operation) =>
      ["failed", "attention"].includes(operation.state),
    ).length;
  const executionDetails = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    if ((executionIssues > 0 || query) && executionDetails.current) {
      executionDetails.current.open = true;
    }
  }, [executionIssues, query]);
  function clearPlan() {
    const next = new URLSearchParams(selected);
    next.delete("plan");
    setSelected(next);
  }
  function resetFilters() {
    setNetwork("all");
    setPlanStatus("all");
    setOperationStatus("all");
    setSearch("");
  }
  useEffect(() => {
    if (selectedPlan) resetFilters();
  }, [selectedPlan]);
  const [diagnostic, setDiagnostic] = useState<TransportDiagnostic | null>(
    null,
  );
  const diagnosticRequest = useRef(0);
  useEffect(
    () => () => {
      diagnosticRequest.current += 1;
    },
    [],
  );
  function closeDiagnostic() {
    diagnosticRequest.current += 1;
    setDiagnostic(null);
  }
  async function inspect(route: string) {
    const request = ++diagnosticRequest.current;
    setDiagnostic({ route });
    try {
      const value = await api.transportStatus(project.key, route);
      if (request !== diagnosticRequest.current) return;
      setDiagnostic({
        route,
        value,
      });
    } catch (failure) {
      if (request !== diagnosticRequest.current) return;
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
      <div className="list-filters">
        <Field
          label="Find activity"
          hint="Search by plan, task, request, command or transaction ID."
        >
          <input
            type="search"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Identifier or command action"
          />
        </Field>
        <Field label="Activity network">
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
      </div>
      {selectedPlan && (
        <Notice>
          Showing funding plan <Address value={selectedPlan} />.{" "}
          <button className="subtle-button" onClick={clearPlan}>
            Show all funding plans
          </button>
        </Notice>
      )}
      <Panel
        title="Funding plans"
        description="Follow each approved budget from reserved funds to the task’s allocation."
      >
        <div className="list-filters">
          <Field label="Funding plan status">
            <select
              value={planStatus}
              onChange={(event) => setPlanStatus(event.target.value)}
            >
              <option value="all">All plans</option>
              <option value="open">In progress</option>
              <option value="attention">Needs attention</option>
              <option value="settled">Funded or closed</option>
            </select>
          </Field>
        </div>
        <p className="filter-summary" role="status">
          {plans.length} of {project.plans.length} funding plans
        </p>
        {plans.length ? (
          plans.map((plan) => (
            <FundingPlan
              key={plan.id}
              project={project}
              plan={plan}
              act={act}
              diagnostic={diagnostic}
              inspect={inspect}
              closeDiagnostic={closeDiagnostic}
            />
          ))
        ) : (
          <Empty
            title={
              selectedPlan
                ? "Funding plan is not visible in this view"
                : project.plans.length
                  ? "No funding plans match these filters"
                  : "No funding plans yet"
            }
            action={
              project.plans.length || selectedPlan ? (
                <button
                  className="button small secondary"
                  onClick={() => {
                    resetFilters();
                    clearPlan();
                  }}
                >
                  Show all plans
                </button>
              ) : (
                <Link className="text-link" to={`${base}/tasks`}>
                  Open tasks & queue →
                </Link>
              )
            }
          >
            {selectedPlan
              ? "The plan may still be indexing or excluded by a filter. Review the full project history or refresh after confirmation."
              : "The manager starts a plan from the head of the approval queue."}
          </Empty>
        )}
      </Panel>
      <details className="disclosure execution-details" ref={executionDetails}>
        <summary>
          Commands & transactions
          <span className="disclosure-meta">
            {commands.length + operations.length} records
            {executionIssues > 0 && ` · ${executionIssues} need attention`}
          </span>
        </summary>
        <p className="review-note">
          Inspect individual network actions and transaction receipts.
          Submission is confirmed separately from execution.
        </p>
        <div className="list-filters">
          <Field label="Command and operation status">
            <select
              value={operationStatus}
              onChange={(event) => setOperationStatus(event.target.value)}
            >
              <option value="all">All operations</option>
              <option value="open">In progress</option>
              <option value="attention">Failed or needs attention</option>
              <option value="settled">Confirmed or invalidated</option>
            </select>
          </Field>
        </div>
        {project.commands.length > 0 && (
          <Panel
            title="Authorized actions"
            description="Each command allows a specific action on its destination network."
          >
            {commands.length ? (
              <div
                className="table-scroll"
                tabIndex={0}
                role="region"
                aria-label="Coordinator commands"
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
                    {commands.map((command) => (
                      <tr key={command.id}>
                        <td>
                          {command.action.replaceAll("_", " ")}
                          <small>
                            <Address value={command.id} />
                          </small>
                          {command.signature_error && (
                            <small>
                              {["succeeded", "invalidated"].includes(
                                command.status,
                              ) && "Previous signing error: "}
                              {command.signature_error}
                            </small>
                          )}
                          {project.plans.some(
                            (plan) => plan.id === command.plan,
                          ) && (
                            <small>
                              <Link
                                className="text-link"
                                to={`${base}/activity?${new URLSearchParams({ plan: command.plan })}`}
                              >
                                View funding plan →
                              </Link>
                            </small>
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
                            ["pending", "unobserved"].includes(
                              command.status,
                            ) && (
                              <button
                                className="button secondary small"
                                onClick={() =>
                                  act({ kind: "invalidate", command })
                                }
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
            ) : (
              <Empty
                title="No commands match these filters"
                action={
                  <button
                    className="button secondary small"
                    onClick={resetFilters}
                  >
                    Clear filters
                  </button>
                }
              />
            )}
          </Panel>
        )}
        <Panel
          title="Network transactions"
          description="Track submissions and their confirmed outcomes."
        >
          {operations.length ? (
            <div
              className="table-scroll"
              tabIndex={0}
              role="region"
              aria-label="Native chain operations"
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
                  {operations.map((operation) => (
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
            <Empty
              title={
                project.operations.length
                  ? "No chain operations match these filters"
                  : "No chain operations recorded"
              }
              action={
                project.operations.length > 0 && (
                  <button
                    className="button secondary small"
                    onClick={resetFilters}
                  >
                    Clear filters
                  </button>
                )
              }
            >
              Worker activity appears here as contracts progress.
            </Empty>
          )}
        </Panel>
      </details>
    </>
  );
}

function FundingPlan({
  project,
  plan,
  act,
  diagnostic,
  inspect,
  closeDiagnostic,
}: {
  project: ProjectDetail;
  plan: PlanView;
  act: (action: ActionTarget) => void;
  diagnostic: TransportDiagnostic | null;
  inspect: (route: string) => Promise<void>;
  closeDiagnostic: () => void;
}) {
  const base = `/app/projects/${encodeURIComponent(project.key)}`;
  const terminal = ["funded", "closed"].includes(plan.status);
  const request = project.requests.find((request) => request.plan === plan.id);
  const canRequeue =
    project.roles.authority &&
    plan.status === "closed" &&
    request?.local_state === "pending" &&
    plan.routes.every(
      (route) => BigInt(route.dispatched) === 0n || BigInt(route.returned) > 0n,
    );
  const recoveryDetails = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    if (plan.requires_attention && !terminal && recoveryDetails.current) {
      recoveryDetails.current.open = true;
    }
  }, [plan.requires_attention, terminal]);
  const received = plan.routes.reduce(
    (total, route) => total + BigInt(route.received),
    0n,
  );
  const progress: Record<string, string> = {
    reserving: "Reserving project funds for this task.",
    preparing: "Preparing transfers to the task’s network.",
    delivering:
      "Waiting for transfers to be confirmed on the destination network.",
    allocating:
      "Funds are ready. The task allocation is awaiting confirmation.",
    settling:
      "The task is funded. Remaining transfers and refunds are being settled.",
    funded: "The task has received its full approved allocation.",
    aborting: "Stopping new transfers and settling funds already sent.",
    closed:
      "The plan is closed. Review the task for its current funding status.",
  };
  return (
    <div id={`funding-plan-${plan.id}`} className="operation-card">
      <div className="operation-title">
        <div>
          <h3>
            <Money value={plan.principal} /> to {chainNames[plan.destination]}
          </h3>
          <p className="review-note" style={{ marginTop: 7 }}>
            {progress[plan.status] ?? "Waiting for the next confirmed update."}
          </p>
          {request && (
            <Link
              className="text-link"
              to={`${base}/tasks?${new URLSearchParams({ chain: request.chain, task: request.task })}`}
            >
              View task {short(request.task, 6)} →
            </Link>
          )}
        </div>
        <Status
          value={
            plan.requires_attention && !terminal ? "attention" : plan.status
          }
        />
      </div>
      {plan.error &&
        (terminal ? (
          <details className="disclosure">
            <summary>Previous recovery context</summary>
            <p>{plan.error}</p>
          </details>
        ) : (
          <Notice tone="error">{plan.error}</Notice>
        ))}
      <div className="inline-meta">
        {plan.routes.length > 0 && (
          <span>
            Transfers received <Money value={received.toString()} />
          </span>
        )}
        {!terminal && (
          <span>
            Funding deadline{" "}
            {new Date(Number(plan.expires) * 1000).toLocaleString()}
          </span>
        )}
      </div>
      <details className="disclosure">
        <summary>
          {plan.routes.length ? "Transfer breakdown" : "Allocation details"}
          {plan.routes.length > 0 && (
            <span className="disclosure-meta">
              {plan.routes.length}{" "}
              {plan.routes.length === 1 ? "route" : "routes"}
            </span>
          )}
        </summary>
        <dl className="definition-list">
          <div>
            <dt>Funding plan</dt>
            <dd>
              <Address value={plan.id} />
            </dd>
          </div>
          <div>
            <dt>Already on the destination network</dt>
            <dd>
              <Money value={plan.local_amount} />
            </dd>
          </div>
          <div>
            <dt>Recovery attempts</dt>
            <dd>
              {plan.attempts} / {project.retry_limit}
            </dd>
          </div>
          {terminal && (
            <div>
              <dt>Funding deadline</dt>
              <dd>{new Date(Number(plan.expires) * 1000).toLocaleString()}</dd>
            </div>
          )}
        </dl>
        {plan.routes.length > 0 && (
          <div
            className="table-scroll"
            tabIndex={0}
            role="region"
            aria-label={`Transfer routes for plan ${short(plan.id, 6)}`}
            style={{ marginTop: 17 }}
          >
            <table>
              <thead>
                <tr>
                  <th>Route</th>
                  <th>Sent</th>
                  <th>Received</th>
                  <th>Returned</th>
                  <th>Still unsettled</th>
                  <th>Transfer cost</th>
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
                        <Money value={route.confirmed_transport_expense} />
                      )}
                    </td>
                    <td>
                      <button
                        className="subtle-button"
                        aria-label={`Provider details for route ${short(route.id, 6)}`}
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
            All funds are already on the task’s network. No transfer between
            networks is needed.
          </p>
        )}
        {diagnostic &&
          plan.routes.some((route) => route.id === diagnostic.route) && (
            <div style={{ marginTop: 16 }}>
              <div className="operation-title">
                <h4>
                  Provider details · <Address value={diagnostic.route} />
                </h4>
                <button className="subtle-button" onClick={closeDiagnostic}>
                  Close details
                </button>
              </div>
              <Notice>
                Provider status is diagnostic. Only verified chain receipts
                change the project’s accounting.
              </Notice>
              {diagnostic.error ? (
                <Notice tone="error">
                  {diagnostic.error}{" "}
                  <button
                    className="subtle-button"
                    onClick={() => void inspect(diagnostic.route)}
                  >
                    Try again
                  </button>
                </Notice>
              ) : diagnostic.value === undefined ? (
                <Loading>Fetching provider status…</Loading>
              ) : (
                <pre>{JSON.stringify(diagnostic.value, null, 2)}</pre>
              )}
            </div>
          )}
      </details>
      {(project.roles.authority || project.roles.owner) &&
        (!terminal || canRequeue) && (
          <details className="disclosure" ref={recoveryDetails}>
            <summary>Recovery actions</summary>
            <p className="review-note" style={{ marginTop: 12 }}>
              Recovery retains already submitted transfers and their receipts.
              It never treats an unknown result as a failed payment.
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
              {canRequeue && (
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
  );
}
