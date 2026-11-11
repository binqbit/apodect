import type { ProjectDetail } from "./api";
import type { ActionTarget } from "./action";
import {
  ChainBadge,
  Empty,
  Money,
  Notice,
  Panel,
  ReferenceContent,
  Status,
  chainNames,
} from "./ui";

export function Funding({
  project,
  act,
}: {
  project: ProjectDetail;
  act: (action: ActionTarget) => void;
}) {
  return (
    <>
      <Panel
        title="Project liquidity"
        description="A deposit credits only confirmed tokens. A withdrawal can use genuinely free project funds."
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
                <th>Eligible for funding</th>
                <th>Local free funds</th>
                <th>Actions</th>
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
                    {chain.local_free === null ? (
                      "Not observed"
                    ) : (
                      <Money value={chain.local_free} />
                    )}
                  </td>
                  <td>
                    <div className="row-actions">
                      {project.roles.local_owners[chain.chain] && (
                        <button
                          className="button small"
                          onClick={() =>
                            act({ kind: "fund", chain: chain.chain })
                          }
                        >
                          Add funds
                        </button>
                      )}
                      {project.roles.owner && (
                        <button
                          className="button small secondary"
                          onClick={() =>
                            act({ kind: "withdraw_free", chain: chain.chain })
                          }
                        >
                          Withdraw free funds
                        </button>
                      )}
                      {!project.roles.local_owners[chain.chain] &&
                        !project.roles.owner && (
                          <span className="muted">Owner wallet required</span>
                        )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Panel>
      <Notice>
        Delivery costs come from the project budget. The agent receives its full
        approved principal; a funding shortfall leaves the request waiting.
      </Notice>
      <Panel
        title="Replenishment requests"
        description="Managers request funding from the owner. An owner response closes the request; actual deposits are tracked separately."
        action={
          project.roles.authority && (
            <button
              className="button secondary small"
              onClick={() => act({ kind: "open_invoice" })}
            >
              Request project funding
            </button>
          )
        }
      >
        {project.invoices.length ? (
          project.invoices.map((invoice) => (
            <div key={invoice.id} className="operation-card">
              <div className="operation-title">
                <h3>Replenishment #{invoice.id}</h3>
                <Status value={invoice.closed ? "closed" : "awaiting owner"} />
              </div>
              <dl className="definition-list">
                <div>
                  <dt>Requested</dt>
                  <dd>
                    <Money value={invoice.amount} />
                  </dd>
                </div>
                <div>
                  <dt>Owner’s intended contribution</dt>
                  <dd>
                    {invoice.intended === null ? (
                      "No response yet"
                    ) : (
                      <Money value={invoice.intended} />
                    )}
                  </dd>
                </div>
                <div>
                  <dt>Confirmed linked deposits</dt>
                  <dd>
                    <Money value={invoice.actual_funding} />
                  </dd>
                </div>
              </dl>
              <ReferenceContent
                project={project.key}
                digest={invoice.purpose}
                label="Purpose"
              />
              {invoice.response_reference && (
                <ReferenceContent
                  project={project.key}
                  digest={invoice.response_reference}
                  label="Owner response"
                />
              )}
              {project.roles.owner && !invoice.closed && (
                <div className="operation-actions">
                  <button
                    className="button small"
                    onClick={() => act({ kind: "respond_invoice", invoice })}
                  >
                    Respond
                  </button>
                </div>
              )}
              {invoice.closed && (
                <div className="operation-actions">
                  {project.chains
                    .filter((chain) => project.roles.local_owners[chain.chain])
                    .map((chain) => (
                      <button
                        key={chain.chain}
                        className="button secondary small"
                        onClick={() =>
                          act({ kind: "fund", invoice, chain: chain.chain })
                        }
                      >
                        Fund on {chainNames[chain.chain]}
                      </button>
                    ))}
                </div>
              )}
            </div>
          ))
        ) : (
          <Empty title="No replenishment requests">
            The manager can request additional project funding here.
          </Empty>
        )}
      </Panel>
    </>
  );
}
