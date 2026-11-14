import { api, ApiError, type Chain } from "./api";
import {
  Children,
  cloneElement,
  isValidElement,
  useEffect,
  useId,
  useState,
  type ReactNode,
} from "react";
import { Link, useLocation } from "react-router-dom";

export const chains = ["ethereum", "solana", "near"] as const;
export const chainNames: Record<Chain, string> = {
  ethereum: "Ethereum",
  solana: "Solana",
  near: "NEAR",
};

export function short(value: string, size = 6): string {
  return value.length > size * 2 + 3
    ? `${value.slice(0, size)}…${value.slice(-size)}`
    : value;
}

export function money(value: string): string {
  const amount = BigInt(value);
  const integer = amount / 1_000_000n;
  const fraction = (amount % 1_000_000n).toString().padStart(6, "0");
  const trimmed = fraction.replace(/0+$/, "").padEnd(2, "0");
  return `${integer.toLocaleString("en-US")}.${trimmed}`;
}

export function baseUnits(value: string): string {
  if (!/^\d+(\.\d{1,6})?$/.test(value.trim())) {
    throw new Error("Enter a USDC amount with up to 6 decimal places.");
  }
  const [whole, fraction = ""] = value.trim().split(".");
  const amount = BigInt(whole) * 1_000_000n + BigInt(fraction.padEnd(6, "0"));
  if (amount > 18_446_744_073_709_551_615n) {
    throw new Error("The amount exceeds the supported token limit.");
  }
  return amount.toString();
}

export function Money({
  value,
  unit = true,
}: {
  value: string;
  unit?: boolean;
}) {
  return (
    <span className="money">
      {money(value)}
      {unit && <small> USDC</small>}
    </span>
  );
}

export function Address({
  value,
  full = false,
}: {
  value: string;
  full?: boolean;
}) {
  const [feedback, setFeedback] = useState("");
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    setFeedback("");
    setExpanded(false);
  }, [value]);
  useEffect(() => {
    if (!feedback) return;
    const timer = window.setTimeout(() => setFeedback(""), 2500);
    return () => window.clearTimeout(timer);
  }, [feedback]);
  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setFeedback("Copied");
    } catch {
      setFeedback("Select the value to copy it");
    }
  }
  return (
    <span className="address-control">
      {full || short(value, 9) === value || feedback.startsWith("Select") ? (
        <span className="address">{value}</span>
      ) : (
        <button
          type="button"
          className="address address-value"
          aria-expanded={expanded}
          aria-label={`${expanded ? "Shorten" : "Show full"} address ${value}`}
          onClick={() => setExpanded((previous) => !previous)}
        >
          {expanded ? value : short(value, 9)}
        </button>
      )}
      <button
        type="button"
        className="copy-button"
        onClick={() => void copy()}
        aria-label={`Copy ${value}`}
        title="Copy full value"
      >
        ⧉
      </button>
      <span className="copy-feedback" role="status">
        {feedback}
      </span>
    </span>
  );
}

export function ChainBadge({ chain }: { chain: Chain }) {
  return (
    <span className={`chain chain-${chain}`}>
      <span aria-hidden="true">
        {chain === "ethereum" ? "◆" : chain === "solana" ? "≋" : "N"}
      </span>
      {chainNames[chain]}
    </span>
  );
}

export function Status({ value }: { value: string }) {
  const label = value.replaceAll("_", " ");
  const positive = [
    "active",
    "ready",
    "funded",
    "confirmed",
    "completed",
    "settled",
  ].includes(value.toLowerCase());
  const danger = ["attention", "failed", "rejected", "out_of_budget"].includes(
    value.toLowerCase(),
  );
  return (
    <span
      className={`status ${positive ? "positive" : danger ? "danger" : "neutral"}`}
    >
      <span aria-hidden="true" />
      {label}
    </span>
  );
}

export function Notice({
  children,
  tone = "info",
}: {
  children: ReactNode;
  tone?: "info" | "error" | "success";
}) {
  return (
    <div
      className={`notice notice-${tone}`}
      role={tone === "error" ? "alert" : "status"}
    >
      {children}
    </div>
  );
}

export function Empty({
  title,
  children,
  action,
}: {
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <span className="empty-mark" aria-hidden="true">
        ↗
      </span>
      <h3>{title}</h3>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

export function Loading({
  children = "Loading confirmed state…",
}: {
  children?: ReactNode;
}) {
  return (
    <div className="loading" role="status">
      <span className="spinner" />
      {children}
    </div>
  );
}

export function GuideLink({
  section,
  children,
}: {
  section?: "owner" | "manager" | "agent" | "funding" | "wallets";
  children: ReactNode;
}) {
  const location = useLocation();
  return (
    <Link
      className="text-link"
      to={`/guide${section ? `#${section}` : ""}`}
      state={{
        returnTo: `${location.pathname}${location.search}${location.hash}`,
        returnState: location.state,
      }}
    >
      {children}
    </Link>
  );
}

export function PageTitle({
  eyebrow,
  title,
  children,
  action,
}: {
  eyebrow?: string;
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="page-title">
      <div>
        {eyebrow && <p className="eyebrow">{eyebrow}</p>}
        <h1>{title}</h1>
        {children && <div className="muted">{children}</div>}
      </div>
      {action}
    </div>
  );
}

export function Panel({
  title,
  description,
  children,
  action,
  className = "",
}: {
  title?: string;
  description?: string;
  children: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <section className={`panel ${className}`}>
      {title && (
        <div className="panel-heading">
          <div>
            <h2>{title}</h2>
            {description && <p>{description}</p>}
          </div>
          {action}
        </div>
      )}
      {children}
    </section>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  const id = useId();
  const hintId = `${id}-hint`;
  // Associate a label with the native control even when an amount adornment
  // wraps it. Supporting text is a description, never part of its name.
  function associate(nodes: ReactNode): ReactNode {
    return Children.map(nodes, (node) => {
      if (
        !isValidElement<{
          children?: ReactNode;
          id?: string;
          "aria-describedby"?: string;
        }>(node)
      )
        return node;
      if (
        typeof node.type === "string" &&
        ["input", "select", "textarea"].includes(node.type)
      ) {
        return cloneElement(node, {
          id,
          "aria-describedby": hint ? hintId : undefined,
        });
      }
      return node.props.children
        ? cloneElement(node, { children: associate(node.props.children) })
        : node;
    });
  }
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      {associate(children)}
      {hint && <small id={hintId}>{hint}</small>}
    </div>
  );
}

export function Brand() {
  return (
    <Link to="/" aria-label="Apodect home" className="brand">
      <svg
        className="brand-mark"
        viewBox="0 0 32 32"
        fill="none"
        aria-hidden="true"
      >
        <path
          d="m11 11 5 5m5-5-5 5m0 0v5"
          stroke="currentColor"
          strokeWidth="2"
        />
        <path
          d="M3 3h8v8H3zm18 0h8v8h-8zm-9 18h8v8h-8z"
          stroke="currentColor"
          strokeWidth="2"
        />
        <path d="m16 12 4 4-4 4-4-4z" fill="currentColor" />
      </svg>
      Apodect
    </Link>
  );
}

export function ErrorState({
  error,
  retry,
}: {
  error: unknown;
  retry?: () => void;
}) {
  const message =
    error instanceof ApiError && error.status < 500 ? error.message : null;
  return (
    <Notice tone="error">
      <strong>Unable to load this view.</strong>
      <p>{message ?? "Try again in a moment, or check the error details."}</p>
      {retry && (
        <button className="button small secondary" onClick={retry}>
          Try again
        </button>
      )}
      {!message && error instanceof Error && (
        <details className="disclosure">
          <summary>Error details</summary>
          <p>{error.message}</p>
        </details>
      )}
    </Notice>
  );
}

/** Discard stale responses when navigation changes; keep the last confirmed
 * view during a refresh rather than flashing invented zero balances. */
export function useLoad<T>(key: string, load: () => Promise<T>) {
  const [state, setState] = useState<{
    key: string;
    data?: T;
    error?: unknown;
    loading: boolean;
  }>({ key, loading: true });
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let current = true;
    setState((previous) => ({
      key,
      data: previous.key === key ? previous.data : undefined,
      loading: true,
    }));
    load().then(
      (data) => current && setState({ key, data, loading: false }),
      (error: unknown) =>
        current &&
        setState((previous) => ({ ...previous, error, loading: false })),
    );
    return () => {
      current = false;
    };
    // The caller supplies a key for every input used by load.
  }, [key, revision]);
  return {
    ...(state.key === key ? state : { key, loading: true }),
    refresh: () => setRevision((n) => n + 1),
  };
}

export function ReferenceContent({
  project,
  digest,
  label,
  compact = false,
}: {
  project: string;
  digest: string;
  label: string;
  compact?: boolean;
}) {
  const result = useLoad(`${project}:${digest}`, () =>
    api.reference(project, digest),
  );
  const unavailable =
    result.error instanceof ApiError &&
    [403, 404].includes(result.error.status);
  if (compact)
    return (
      <span className="task-title-text">
        {result.data?.content.split("\n")[0] ?? label}
      </span>
    );
  return (
    <div style={{ marginTop: 15 }}>
      <p className="section-label" style={{ marginTop: 0 }}>
        {label}
      </p>
      {result.data ? (
        <div className="metadata">{result.data.content}</div>
      ) : result.loading ? (
        <p className="review-note">Loading explanation…</p>
      ) : (
        <p className="review-note">
          {unavailable
            ? "This explanation is private or was recorded outside the platform."
            : "The explanation is temporarily unavailable."}
        </p>
      )}
      <details className="disclosure">
        <summary>On-chain reference</summary>
        <div className="disclosure-body">
          <Address value={digest} full />
        </div>
      </details>
    </div>
  );
}
