import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import type { ProjectDetail } from "./api";
import type { ActionTarget } from "./action";
import {
  ChainBadge,
  Empty,
  Field,
  Money,
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
        description="Confirmed USDC available for task funding and owner withdrawals."
      >
        <div className="network-balances funding-balances">
          {project.chains.map((chain) => (
            <div className="network-balance" key={chain.chain}>
              <ChainBadge chain={chain.chain} />
              <strong>
                <Money value={chain.available} />
              </strong>
              <p className="review-note">Available to fund</p>
              <div className="row-actions">
                {project.roles.local_owners[chain.chain] && (
                  <button
                    className="button small"
                    onClick={() => act({ kind: "fund", chain: chain.chain })}
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
                    Withdraw
                  </button>
                )}
                {!project.roles.local_owners[chain.chain] && (
                  <Link
                    className="text-link"
                    to="/app/account"
                    state={{ returnTo, chain: chain.chain }}
                  >
                    Link owner wallet →
                  </Link>
                )}
              </div>
            </div>
          ))}
        </div>
        <details className="disclosure">
          <summary>How available funds are calculated</summary>
          <p className="review-note">
            Available funds exclude money reserved for tasks or pending
            transfers. Network balances may differ while confirmations are being
            processed.
          </p>
          <dl className="definition-list">
            {project.chains.map((chain) => (
              <div key={chain.chain}>
                <dt>{chainNames[chain.chain]} local free balance</dt>
                <dd>
                  {chain.local_free === null ? (
                    "Awaiting confirmation"
                  ) : (
                    <Money value={chain.local_free} />
                  )}
                </dd>
              </div>
            ))}
          </dl>
        </details>
        <details className="disclosure">
          <summary>Transfer costs and owner permissions</summary>
          <p className="review-note">
            The project pays transfer costs. Agents receive the full approved
            amount; a funding shortfall leaves their request waiting.
          </p>
          <p className="review-note">
            Add funds with the owner wallet on that network. Withdrawals require
            the NEAR owner’s signature and go to the project’s registered owner
            recipient on the selected network. Review the recipient before
            signing.
          </p>
        </details>
      </Panel>
      <Panel
        title="Owner funding requests"
        description="A manager requests funds; the owner responds and deposits separately. A closed request does not mean funds have arrived."
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
            <Field label="Funding request status">
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
                <h3>Funding request #{invoice.id}</h3>
                <Status
                  value={invoice.closed ? "owner responded" : "awaiting owner"}
                />
              </div>
              <dl className="definition-list">
                <div>
                  <dt>Requested</dt>
                  <dd>
                    <Money value={invoice.amount} />
                  </dd>
                </div>
                <div>
                  <dt>Owner plans to add</dt>
                  <dd>
                    {invoice.intended === null ? (
                      "No response yet"
                    ) : (
                      <Money value={invoice.intended} />
                    )}
                  </dd>
                </div>
                <div>
                  <dt>Received for this request</dt>
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
                ? "No funding requests match this status"
                : "No owner funding requests"
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
