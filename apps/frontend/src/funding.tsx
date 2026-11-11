import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import type { ProjectDetail } from "./api";
import type { ActionTarget } from "./action";
import {
  ChainBadge,
  Empty,
  Field,
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
  const location = useLocation();
  const returnTo = `${location.pathname}${location.search}${location.hash}`;
  const [status, setStatus] = useState("all");
  const invoices = [...project.invoices]
    .reverse()
    .filter(
      (invoice) =>
        status === "all" ||
        (status === "open" ? !invoice.closed : invoice.closed),
    );
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
          aria-label="Project liquidity by network"
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
                          disabled={BigInt(chain.available) === 0n}
                          title={
                            BigInt(chain.available) === 0n
                              ? "No eligible funds are available on this network."
                              : undefined
                          }
                          onClick={() =>
                            act({ kind: "withdraw_free", chain: chain.chain })
                          }
                        >
                          Withdraw free funds
                        </button>
                      )}
                      {!project.roles.local_owners[chain.chain] && (
                        <Link
                          className="text-link"
                          to="/app/account"
                          state={{ returnTo, chain: chain.chain }}
                        >
                          Link {chainNames[chain.chain]} owner wallet to add
                          funds →
                        </Link>
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
        <p>
          The local owner wallet deposits USDC. Project withdrawals require the
          NEAR owner’s signature and send funds to the registered owner
          recipient on the chosen network.
        </p>
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
        {project.invoices.length > 0 && (
          <div className="list-filters">
            <Field label="Replenishment status">
              <select
                value={status}
                onChange={(event) => setStatus(event.target.value)}
              >
                <option value="all">All requests</option>
                <option value="open">Awaiting owner response</option>
                <option value="closed">Closed requests</option>
              </select>
            </Field>
          </div>
        )}
        {invoices.length ? (
          invoices.map((invoice) => (
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
                  {!project.chains.some(
                    (chain) => project.roles.local_owners[chain.chain],
                  ) && (
                    <Link
                      className="text-link"
                      to="/app/account"
                      state={{ returnTo }}
                    >
                      Link a local owner wallet to fund this request →
                    </Link>
                  )}
                </div>
              )}
            </div>
          ))
        ) : (
          <Empty
            title={
              project.invoices.length
                ? "No replenishment requests match this status"
                : "No replenishment requests"
            }
            action={
              project.invoices.length > 0 && (
                <button
                  className="button secondary small"
                  onClick={() => setStatus("all")}
                >
                  Show all requests
                </button>
              )
            }
          >
            {project.invoices.length
              ? "Choose another status to see the remaining requests."
              : "The manager can request additional project funding here."}
          </Empty>
        )}
      </Panel>
    </>
  );
}
