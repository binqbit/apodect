import { Link } from "react-router-dom";
import type { Chain, FundingCosts, UnsignedIntent } from "./api";
import {
  copy,
  amountKinds,
  purposeKinds,
  expiringKinds,
  displayNative,
  type ActionKind,
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
  tokenAccounts,
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
  tokenAccounts: { address: string; amount: string }[];
  needsStorage: boolean;
  signingChain: Chain;
  onClose: () => void;
}) {
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
    <fieldset disabled={busy} className="form-stack">
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
          <Link className="text-link" to="/app/account" onClick={onClose}>
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
            target.task && ["withdraw", "release", "reclaim"].includes(kind)
              ? `Confirmed allocation available: ${money(target.task.available)} USDC`
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
        </Field>
      )}
      {target.request && (
        <div className="review">
          <dl className="definition-list">
            <div>
              <dt>Approved / requested principal</dt>
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
              : "Explain the work and expected use of funds. The explanation is saved with an immutable hash; it does not restrict downstream purchases."
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
              : "Saved as an immutable explanation. A report does not replace confirmed token returns."
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
        <label className="checkbox-field">
          <input
            type="checkbox"
            checked={existingReference}
            onChange={(event) =>
              change("existingReference", event.target.checked)
            }
          />
          Use an existing on-chain reference instead of text
        </label>
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
          hint="This bounds route execution. Delivery costs still come from the project, separately from the approved principal."
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
          {tokenError ? (
            <Notice tone="error">{tokenError}</Notice>
          ) : !tokenAccounts.length ? (
            <Loading>Reading wallet token accounts…</Loading>
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
    </fieldset>
  );
}
export function ActionReview({
  kind,
  intent,
  financialAmount,
  costs,
  preparedExpires,
}: {
  kind: ActionKind;
  intent: UnsignedIntent;
  financialAmount: string | null;
  costs: FundingCosts | null;
  preparedExpires: number | null;
}) {
  return (
    <div className="review">
      <h3>Review before signing</h3>
      <dl className="definition-list">
        <div>
          <dt>Action</dt>
          <dd>{copy[kind].title}</dd>
        </div>
        <div>
          <dt>Signing network</dt>
          <dd>
            <ChainBadge chain={intent.chain} />
          </dd>
        </div>
        <div>
          <dt>Your wallet</dt>
          <dd>
            <Address value={intent.actor} />
          </dd>
        </div>
        {financialAmount && (
          <div>
            <dt>
              {kind === "respond_invoice"
                ? "Intended contribution"
                : "Principal / amount"}
            </dt>
            <dd>
              <Money value={financialAmount} />
            </dd>
          </div>
        )}
        {costs && (
          <>
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
        {preparedExpires && (
          <div>
            <dt>Valid until</dt>
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
      <p className="review-note">
        {copy[kind].description} The wallet shows native gas fees before
        signing. Each prerequisite transaction must finalize before the next
        step.
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
      <details>
        <summary>Exact wallet instructions</summary>
        <pre>{JSON.stringify(intent, null, 2)}</pre>
      </details>
    </div>
  );
}
