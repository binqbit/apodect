import { Link, useLocation } from "react-router-dom";
import type { Chain, FundingCosts, ProjectDetail, UnsignedIntent } from "./api";
import {
  amountKinds,
  purposeKinds,
  expiringKinds,
  displayNative,
  type ActionTarget,
  type ActionFieldsValue,
  type ChangeField,
} from "./action-model";
import {
  Address,
  ChainBadge,
  Field,
  Loading,
  Money,
  Notice,
  chainNames,
  chains,
  money,
} from "./ui";

export function ActionFields({
  values,
  change,
  target,
  busy,
  eligibleWallets,
  needsToken,
  tokenError,
  tokenLoading,
  tokenAccounts,
  onTokenRetry,
  amountLimit,
  needsStorage,
  signingChain,
  onClose,
}: {
  values: ActionFieldsValue;
  change: ChangeField;
  target: ActionTarget;
  busy: boolean;
  eligibleWallets: { address: string }[];
  needsToken: boolean;
  tokenError: string;
  tokenLoading: boolean;
  tokenAccounts: { address: string; amount: string }[];
  onTokenRetry: () => void;
  amountLimit: string | null;
  needsStorage: boolean;
  signingChain: Chain;
  onClose: () => void;
}) {
  const location = useLocation();
  const { kind } = target;
  const {
    chain,
    actor,
    amount,
    purpose,
    report,
    existingReference,
    manager,
    attempts,
    slippage,
    minutes,
    storage,
    reason,
    unverified,
    tokenAccount,
  } = values;
  return (
    <fieldset
      disabled={busy}
      className="form-stack"
      onInvalidCapture={(event) => {
        const settings = (event.target as HTMLElement).closest("details");
        if (settings) settings.open = true;
      }}
    >
      {kind === "create_task" && (
        <Field
          label="Execution network"
          hint="The task and its request will be created locally on this network."
        >
          <select
            value={chain}
            onChange={(event) => change("chain", event.target.value as Chain)}
          >
            {chains.map((value) => (
              <option key={value} value={value}>
                {chainNames[value]}
              </option>
            ))}
          </select>
        </Field>
      )}
      <Field label={`Signing wallet · ${chainNames[signingChain]}`}>
        <select
          value={actor}
          onChange={(event) => {
            change("actor", event.target.value);
            change("tokenAccount", "");
          }}
          required
        >
          {eligibleWallets.length === 0 && (
            <option value="">No eligible linked wallet</option>
          )}
          {eligibleWallets.map((item) => (
            <option key={item.address} value={item.address}>
              {item.address}
            </option>
          ))}
        </select>
      </Field>
      {eligibleWallets.length === 0 && (
        <Notice>
          Link a wallet with the required role in{" "}
          <Link
            className="text-link"
            to="/app/account"
            state={{
              returnTo: location.pathname + location.search + location.hash,
              chain: signingChain,
            }}
            onClick={onClose}
          >
            Account & wallets →
          </Link>
        </Notice>
      )}
      {amountKinds.includes(kind) && (
        <Field
          label={
            kind === "respond_invoice" ? "Intended contribution" : "Amount"
          }
          hint={
            amountLimit !== null
              ? `Confirmed maximum: ${money(amountLimit)} USDC`
              : "USDC · Up to 6 decimal places"
          }
        >
          <div className="amount-input">
            <input
              inputMode="decimal"
              value={amount}
              onChange={(event) => change("amount", event.target.value)}
              placeholder="0.00"
              required
            />
            <span>USDC</span>
          </div>
          {amountLimit !== null && BigInt(amountLimit) > 0n && (
            <button
              type="button"
              className="text-link"
              onClick={() =>
                change("amount", money(amountLimit).replaceAll(",", ""))
              }
            >
              Use confirmed maximum
            </button>
          )}
        </Field>
      )}
      {target.request && (
        <div className="review">
          <dl className="definition-list">
            <div>
              <dt>
                {kind === "apply" ? "Approved budget" : "Requested budget"}
              </dt>
              <dd>
                <Money value={target.request.amount} />
              </dd>
            </div>
            <div>
              <dt>Execution network</dt>
              <dd>
                <ChainBadge chain={chain} />
              </dd>
            </div>
          </dl>
        </div>
      )}
      {purposeKinds.includes(kind) && (
        <Field
          label={
            existingReference ? "Purpose reference" : "What is this budget for?"
          }
          hint={
            existingReference
              ? "An existing nonzero 32-byte reference, as 64 hexadecimal characters."
              : "Explain the work and use of funds. This record cannot be edited later and does not restrict what the agent can purchase."
          }
        >
          {existingReference ? (
            <input
              value={purpose}
              onChange={(event) => change("purpose", event.target.value)}
              placeholder="64 hexadecimal characters"
              minLength={64}
              maxLength={66}
              required
            />
          ) : (
            <textarea
              value={purpose}
              onChange={(event) => change("purpose", event.target.value)}
              placeholder="Describe the task, its purpose and the requested budget…"
              maxLength={16000}
              required
            />
          )}
        </Field>
      )}
      {["finalize", "respond_invoice"].includes(kind) && (
        <Field
          label={
            kind === "finalize"
              ? `Completion report${existingReference ? " reference" : ""} (optional)`
              : `Owner response${existingReference ? " reference" : ""} (optional)`
          }
          hint={
            existingReference
              ? "An existing 32-byte reference."
              : kind === "finalize"
                ? "This record cannot be edited later. Return unused funds separately before completing the task."
                : "This record cannot be edited later. Transfer your contribution separately after responding."
          }
        >
          {existingReference ? (
            <input
              value={report}
              onChange={(event) => change("report", event.target.value)}
              placeholder="64 hexadecimal characters"
              maxLength={66}
            />
          ) : (
            <textarea
              value={report}
              onChange={(event) => change("report", event.target.value)}
              placeholder={
                kind === "finalize"
                  ? "What was completed? Include relevant result references."
                  : "Explain your contribution or next steps."
              }
              maxLength={16000}
            />
          )}
        </Field>
      )}
      {(purposeKinds.includes(kind) ||
        ["finalize", "respond_invoice"].includes(kind)) && (
        <details className="disclosure">
          <summary>
            {existingReference
              ? "Reference input enabled"
              : "Have an existing on-chain reference?"}
          </summary>
          <div className="disclosure-body">
            <label className="checkbox-field">
              <input
                type="checkbox"
                checked={existingReference}
                onChange={(event) =>
                  change("existingReference", event.target.checked)
                }
              />
              Use a reference instead of writing text
            </label>
            <p className="review-note">
              Use this only if you already have the reference for a saved
              explanation. Switching input format keeps your current entry.
            </p>
          </div>
        </details>
      )}
      {kind === "set_authority" && (
        <Field label="New manager’s NEAR account">
          <input
            required
            value={manager}
            onChange={(event) => change("manager", event.target.value)}
            placeholder="manager.near"
          />
        </Field>
      )}
      {kind === "set_retry_limit" && (
        <Field label="Maximum recovery attempts" hint="From 1 to 20 attempts.">
          <input
            type="number"
            required
            min="1"
            max="20"
            step="1"
            value={attempts}
            onChange={(event) => change("attempts", event.target.value)}
          />
        </Field>
      )}
      {kind === "apply" && (
        <Field
          label="Maximum route slippage (%)"
          hint="From 0 to 10%, with at most two decimals. Delivery costs come from the project, separately from the approved principal."
        >
          <input
            inputMode="decimal"
            required
            value={slippage}
            onChange={(event) => change("slippage", event.target.value)}
          />
        </Field>
      )}
      {needsToken && (
        <Field label="Your USDC token account">
          {tokenLoading ? (
            <Loading>Reading wallet token accounts…</Loading>
          ) : tokenError ? (
            <Notice tone="error">
              {tokenError}
              <button
                type="button"
                className="button small secondary"
                onClick={onTokenRetry}
              >
                Refresh token accounts
              </button>
            </Notice>
          ) : !tokenAccounts.length ? (
            <Notice>
              Your USDC receiving account will be created in the same
              transaction as this withdrawal. Review its SOL rent before
              signing.
            </Notice>
          ) : (
            <select
              value={tokenAccount}
              onChange={(event) => change("tokenAccount", event.target.value)}
              required
            >
              {tokenAccounts.map((item) => (
                <option key={item.address} value={item.address}>
                  {item.address} · {money(item.amount)} USDC
                </option>
              ))}
            </select>
          )}
        </Field>
      )}
      {kind === "retry" && (
        <>
          <Field label="Known failure (optional)">
            <textarea
              maxLength={512}
              value={reason}
              onChange={(event) => change("reason", event.target.value)}
            />
          </Field>
          <Field label="What remains unverified (optional)">
            <textarea
              maxLength={512}
              value={unverified}
              onChange={(event) => change("unverified", event.target.value)}
            />
          </Field>
        </>
      )}
      {(expiringKinds.includes(kind) || needsStorage) && (
        <details className="disclosure">
          <summary>
            <span>Transaction settings</span>
            <small>
              {[
                expiringKinds.includes(kind) && `Valid for ${minutes} min`,
                needsStorage && `${storage} NEAR storage prepayment`,
              ]
                .filter(Boolean)
                .join(" · ")}
            </small>
          </summary>
          <div className="disclosure-body form-stack">
            {expiringKinds.includes(kind) && (
              <Field
                label="Command validity (minutes)"
                hint="Unexecuted commands expire after this period; an already submitted transfer still needs settlement."
              >
                <input
                  type="number"
                  min="1"
                  max="1440"
                  required
                  value={minutes}
                  onChange={(event) => change("minutes", event.target.value)}
                />
              </Field>
            )}
            {needsStorage && (
              <Field
                label="Storage prepayment (NEAR)"
                hint="The contract charges actual storage and refunds unused prepayment. Native network gas is additional."
              >
                <input
                  inputMode="decimal"
                  value={storage}
                  onChange={(event) => change("storage", event.target.value)}
                  required
                />
              </Field>
            )}
          </div>
        </details>
      )}
    </fieldset>
  );
}
export function ActionReview({
  project,
  target,
  values,
  intent,
  financialAmount,
  costs,
  preparedExpires,
}: {
  project: ProjectDetail;
  target: ActionTarget;
  values: ActionFieldsValue;
  intent: UnsignedIntent;
  financialAmount: string | null;
  costs: FundingCosts | null;
  preparedExpires: number | null;
}) {
  const { kind } = target;
  const domain = project.chains.find((view) => view.chain === values.chain);
  const executionChain =
    target.task?.chain ??
    target.request?.chain ??
    target.command?.chain ??
    target.plan?.destination ??
    (kind === "fund" || kind === "withdraw_free" || kind === "create_task"
      ? values.chain
      : null);
  const receiver =
    kind === "fund" || kind === "return_funds"
      ? domain?.vault
      : kind === "withdraw"
        ? (intent.chain === "solana"
            ? intent.token_account_creation?.address
            : undefined) ||
          values.tokenAccount ||
          values.actor
        : kind === "withdraw_free"
          ? domain?.owner_recipient
          : null;
  return (
    <div className="review">
      <h3>Review before signing</h3>
      {financialAmount && (
        <div className="review-primary">
          <span>
            {kind === "respond_invoice"
              ? "Intended contribution"
              : target.request
                ? kind === "apply"
                  ? "Approved budget"
                  : "Requested budget"
                : "Amount"}
          </span>
          <Money value={financialAmount} />
        </div>
      )}
      <dl className="definition-list">
        <div>
          <dt>Signing network</dt>
          <dd>
            <ChainBadge chain={intent.chain} />
          </dd>
        </div>
        {executionChain && executionChain !== intent.chain && (
          <div>
            <dt>Execution network</dt>
            <dd>
              <ChainBadge chain={executionChain} />
            </dd>
          </div>
        )}
        <div>
          <dt>Your wallet</dt>
          <dd>
            <Address value={intent.actor} full />
          </dd>
        </div>
        {receiver && (
          <div>
            <dt>Funds recipient</dt>
            <dd>
              <Address value={receiver} full />
            </dd>
          </div>
        )}
        {target.task && (
          <div>
            <dt>Task</dt>
            <dd>
              <Address value={target.task.id} />
            </dd>
          </div>
        )}
        {target.request && (
          <div>
            <dt>Budget request</dt>
            <dd>
              <Address value={target.request.id} />
            </dd>
          </div>
        )}
        {target.invoice && (
          <div>
            <dt>Replenishment request</dt>
            <dd>#{target.invoice.id}</dd>
          </div>
        )}
        {target.plan && (
          <div>
            <dt>Funding plan</dt>
            <dd>
              <Address value={target.plan.id} />
            </dd>
          </div>
        )}
        {target.command && (
          <>
            <div>
              <dt>Coordinator command</dt>
              <dd>
                <Address value={target.command.id} />
              </dd>
            </div>
            <div>
              <dt>Original action</dt>
              <dd>{target.command.action.replaceAll("_", " ")}</dd>
            </div>
            {target.command.task && (
              <div>
                <dt>Original task</dt>
                <dd>
                  <Address value={target.command.task} />
                </dd>
              </div>
            )}
            {target.command.request && (
              <div>
                <dt>Original budget request</dt>
                <dd>
                  <Address value={target.command.request} />
                </dd>
              </div>
            )}
            <div>
              <dt>Original command amount</dt>
              <dd>
                <Money value={target.command.amount} />
              </dd>
            </div>
            <div>
              <dt>Original command deadline</dt>
              <dd>
                {new Date(
                  Number(target.command.expires) * 1000,
                ).toLocaleString()}
              </dd>
            </div>
          </>
        )}
        {kind === "set_authority" && (
          <>
            <div>
              <dt>Current manager</dt>
              <dd>{project.authority}</dd>
            </div>
            <div>
              <dt>New manager</dt>
              <dd>{values.manager.trim()}</dd>
            </div>
          </>
        )}
        {kind === "set_retry_limit" && (
          <div>
            <dt>Recovery attempt limit</dt>
            <dd>
              {project.retry_limit} → {values.attempts}
            </dd>
          </div>
        )}
        {kind === "finalize" && target.task && (
          <div>
            <dt>Recorded expense on completion</dt>
            <dd>
              <Money
                value={(
                  BigInt(target.task.issued) - BigInt(target.task.recovered)
                ).toString()}
              />
            </dd>
          </div>
        )}
        {costs && (
          <>
            <div>
              <dt>Maximum route slippage</dt>
              <dd>{values.slippage}%</dd>
            </div>
            <div>
              <dt>Local allocation</dt>
              <dd>
                <Money value={costs.local_amount} />
              </dd>
            </div>
            <div>
              <dt>Expected delivery cost</dt>
              <dd>
                <Money value={costs.expected_transport_cost} />
              </dd>
            </div>
            <div>
              <dt>Maximum delivery cost</dt>
              <dd>
                <Money value={costs.maximum_transport_cost} />
              </dd>
            </div>
            <div>
              <dt>Maximum project cost</dt>
              <dd>
                <Money
                  value={(
                    BigInt(costs.principal) +
                    BigInt(costs.maximum_transport_cost)
                  ).toString()}
                />
              </dd>
            </div>
          </>
        )}
        {intent.chain === "solana" && intent.token_account_creation && (
          <>
            <div>
              <dt>New USDC token account</dt>
              <dd>
                <Address value={intent.token_account_creation.address} />
              </dd>
            </div>
            <div>
              <dt>Maximum receiving-account deposit</dt>
              <dd>
                {displayNative(intent.token_account_creation.rent_lamports, 9)}{" "}
                SOL
              </dd>
            </div>
          </>
        )}
        {preparedExpires && (
          <div>
            <dt>
              {kind === "invalidate"
                ? "Invalidation valid until"
                : "Valid until"}
            </dt>
            <dd>{new Date(preparedExpires * 1000).toLocaleString()}</dd>
          </div>
        )}
        {intent.chain === "near" && (
          <div>
            <dt>Attached native deposits</dt>
            <dd>
              {displayNative(
                intent.transactions
                  .reduce((sum, tx) => sum + BigInt(tx.deposit), 0n)
                  .toString(),
              )}{" "}
              NEAR
            </dd>
          </div>
        )}
        <div>
          <dt>Wallet transactions</dt>
          <dd>
            {intent.chain === "solana" ? "1" : intent.transactions.length}
          </dd>
        </div>
      </dl>
      {purposeKinds.includes(kind) && values.purpose.trim() && (
        <div>
          <p className="section-label">
            {values.existingReference ? "Purpose reference" : "Purpose"}
          </p>
          <div className="metadata">{values.purpose.trim()}</div>
        </div>
      )}
      {["finalize", "respond_invoice"].includes(kind) &&
        values.report.trim() && (
          <div>
            <p className="section-label">
              {kind === "finalize" ? "Completion report" : "Owner response"}
              {values.existingReference ? " reference" : ""}
            </p>
            <div className="metadata">{values.report.trim()}</div>
          </div>
        )}
      {kind === "retry" &&
        (values.reason.trim() || values.unverified.trim()) && (
          <div>
            <p className="section-label">Recovery context</p>
            {values.reason.trim() && (
              <p className="metadata">{values.reason.trim()}</p>
            )}
            {values.unverified.trim() && (
              <p className="metadata">Unverified: {values.unverified.trim()}</p>
            )}
          </div>
        )}
      {kind === "finalize" && (
        <Notice>
          Completion is final. Return any unused wallet funds first; completed
          tasks cannot accept further returns or budget requests.
        </Notice>
      )}
      <p className="review-note">
        The wallet shows native gas fees before signing. Each prerequisite
        transaction must finalize before the next step.
      </p>
      {intent.chain === "ethereum" && intent.transactions.length > 1 && (
        <Notice>
          USDC allowance changes are included before the vault action. An
          allowance alone does not deposit or return funds.
        </Notice>
      )}
      {costs?.warnings.map((warning) => (
        <Notice key={warning.route}>
          Quoted route fee <Money value={warning.fee} /> exceeds the{" "}
          {warning.threshold_bps / 100}% warning threshold. The full principal
          remains unchanged.
        </Notice>
      ))}
      <details className="disclosure">
        <summary>Contract & wallet instructions</summary>
        <div className="disclosure-body">
          <dl className="definition-list">
            <div>
              <dt>Project coordinator</dt>
              <dd>
                <Address value={project.coordinator} />
              </dd>
            </div>
          </dl>
          <pre>{JSON.stringify(intent, null, 2)}</pre>
        </div>
      </details>
    </div>
  );
}
