import type {
  Chain,
  CommandView,
  CoordinatorAction,
  InvoiceView,
  LocalAction,
  PlanView,
  ProfileLookup,
  ProjectDetail,
  RequestView,
  TaskView,
} from "./api";

export type ActionKind =
  | "create_task"
  | "request_budget"
  | "withdraw"
  | "return_funds"
  | "release"
  | "finalize"
  | "fund"
  | "approve"
  | "reject"
  | "apply"
  | "pause"
  | "continue"
  | "cancel"
  | "reclaim"
  | "restore"
  | "out_of_budget"
  | "withdraw_free"
  | "open_invoice"
  | "respond_invoice"
  | "set_authority"
  | "set_retry_limit"
  | "abort"
  | "retry"
  | "resume_plan"
  | "requeue"
  | "invalidate";
export interface ActionTarget {
  kind: ActionKind;
  chain?: Chain;
  task?: TaskView;
  request?: RequestView;
  plan?: PlanView;
  invoice?: InvoiceView;
  command?: CommandView;
}

/** Keep an open action tied to the latest indexed version of its selected record. */
export function currentActionTarget(
  project: ProjectDetail,
  target: ActionTarget,
): ActionTarget {
  return {
    ...target,
    task: target.task
      ? project.tasks.find(
          (task) =>
            task.chain === target.task!.chain && task.id === target.task!.id,
        )
      : undefined,
    request: target.request
      ? project.requests.find(
          (request) =>
            request.chain === target.request!.chain &&
            request.id === target.request!.id,
        )
      : undefined,
    plan: target.plan
      ? project.plans.find((plan) => plan.id === target.plan!.id)
      : undefined,
    invoice: target.invoice
      ? project.invoices.find((invoice) => invoice.id === target.invoice!.id)
      : undefined,
    command: target.command
      ? project.commands.find((command) => command.id === target.command!.id)
      : undefined,
  };
}

export function actionAmountLimit(
  project: ProjectDetail,
  target: ActionTarget,
  chain: Chain,
): string | null {
  if (target.task && ["withdraw", "release", "reclaim"].includes(target.kind))
    return target.task.available;
  if (target.task && target.kind === "return_funds") {
    const task = target.task;
    return (
      BigInt(task.issued) -
      BigInt(task.recovered) -
      BigInt(task.available) -
      BigInt(task.pending_outflow)
    ).toString();
  }
  if (target.kind === "withdraw_free")
    return (
      project.chains.find((view) => view.chain === chain)?.available ?? null
    );
  return null;
}

export const copy: Record<ActionKind, { title: string; description: string }> =
  {
    create_task: {
      title: "Create a task",
      description:
        "Request a budget on the network where the work will happen. The manager reviews it before any funds are allocated.",
    },
    request_budget: {
      title: "Request more budget",
      description:
        "A new request follows the ordinary approval queue. Rejecting additional funding stops new withdrawals for this task.",
    },
    withdraw: {
      title: "Withdraw task funds",
      description:
        "Transfer confirmed allocation to the agent’s own wallet. Once withdrawn, the project cannot freeze or automatically recover those tokens.",
    },
    return_funds: {
      title: "Return wallet funds",
      description:
        "Transfer unused USDC back to the project. The returned amount becomes free project funds and does not refill this task’s allowance.",
    },
    release: {
      title: "Release unused allocation",
      description:
        "Return controlled funds from the task to the project. These tokens have not left the vault.",
    },
    finalize: {
      title: "Complete the task",
      description:
        "First return wallet leftovers and release the controlled allocation. Completion records issued funds minus recoveries as expense and closes the task.",
    },
    fund: {
      title: "Add project funds",
      description:
        "Transfer USDC from the local owner wallet into this project’s vault. Funding becomes eligible only after chain confirmation and synchronization.",
    },
    approve: {
      title: "Approve budget request",
      description:
        "Approval fixes the amount and places this request in the funding queue. It does not reserve money or make funds ready to spend.",
    },
    reject: {
      title: "Reject budget request",
      description:
        "The request will not receive funding. Rejecting a supplement stops the task’s further withdrawals while existing controlled funds remain occupied.",
    },
    apply: {
      title: "Review funding plan",
      description:
        "Fund the full request at the head of the queue. The project pays delivery costs in addition to the agent’s approved principal.",
    },
    pause: {
      title: "Pause withdrawals",
      description:
        "Block new withdrawals from this task. Its controlled allocation remains occupied. This does not reverse transfers already accepted or seize external wallet funds.",
    },
    continue: {
      title: "Continue the task",
      description:
        "Remove the owner pause using the available controlled allocation. This does not add budget or undo a stopped task state.",
    },
    cancel: {
      title: "Cancel before first use",
      description:
        "Close this task and release its controlled allocation. Cancellation is possible only before the first accepted withdrawal.",
    },
    reclaim: {
      title: "Reclaim paused allocation",
      description:
        "Move part of a paused task’s confirmed controlled allocation back to free project funds. External wallet funds and pending transfers are excluded.",
    },
    restore: {
      title: "Restore task allocation",
      description:
        "Create an approved restoration request in the ordinary funding queue. Funding it does not remove the owner pause.",
    },
    out_of_budget: {
      title: "Mark task out of budget",
      description:
        "Stop this task and remove its live request from the queue. Its existing allocation stays occupied until explicitly returned or reclaimed. This task cannot be reopened.",
    },
    withdraw_free: {
      title: "Withdraw free project funds",
      description:
        "Send genuinely free funds to the owner’s registered recipient on this network. Reserved, allocated and uncertain amounts are unavailable.",
    },
    open_invoice: {
      title: "Request project funding",
      description:
        "Ask the owner to replenish the project. This request does not reserve funds or alter an agent’s approved amount.",
    },
    respond_invoice: {
      title: "Respond to funding request",
      description:
        "Record the intended contribution and close this request, including for a partial or zero contribution. Transfer the actual funds separately afterwards.",
    },
    set_authority: {
      title: "Replace project manager",
      description:
        "Choose a person by username or enter their NEAR account. Confirming this action replaces the current manager; existing approved tasks and funding deliveries continue.",
    },
    set_retry_limit: {
      title: "Change recovery limit",
      description:
        "Set how many explicit recovery attempts a funding plan can make. An uncertain payment is never automatically replaced.",
    },
    abort: {
      title: "Stop and settle the plan",
      description:
        "Stop new funding actions and begin safe cleanup. Existing receipts still reconcile, and occupied funds release only after confirmed settlement.",
    },
    retry: {
      title: "Retry verification",
      description:
        "Record another verification attempt while keeping unresolved transfers occupied. This counts toward the recovery limit and never authorizes a duplicate payment.",
    },
    resume_plan: {
      title: "Resume funding plan",
      description:
        "Resume a stopped plan only when its confirmed state permits it. Existing reservations and receipts remain part of the same plan.",
    },
    requeue: {
      title: "Requeue settled request",
      description:
        "Reuse the approval only after the old plan has closed and actual refunds have been confirmed. Unresolved old funds stay unavailable.",
    },
    invalidate: {
      title: "Invalidate unexecuted command",
      description:
        "Prevent this command from executing through its on-chain invalidation mechanism. Invalidation does not reverse a command that already executed.",
    },
  };
export const localKinds: ActionKind[] = [
  "create_task",
  "request_budget",
  "withdraw",
  "return_funds",
  "release",
  "finalize",
  "fund",
];
export const amountKinds: ActionKind[] = [
  "create_task",
  "request_budget",
  "withdraw",
  "return_funds",
  "release",
  "fund",
  "reclaim",
  "restore",
  "withdraw_free",
  "open_invoice",
  "respond_invoice",
];
export const purposeKinds: ActionKind[] = [
  "create_task",
  "request_budget",
  "restore",
  "open_invoice",
];
export const expiringKinds: ActionKind[] = [
  "apply",
  "reject",
  "pause",
  "continue",
  "cancel",
  "reclaim",
  "restore",
  "out_of_budget",
  "withdraw_free",
  "invalidate",
];
export const ownerKinds: ActionKind[] = [
  "pause",
  "continue",
  "cancel",
  "reclaim",
  "withdraw_free",
  "respond_invoice",
  "set_authority",
  "set_retry_limit",
];

export function reference(value: string, optional = false): string | null {
  if (!value.trim() && optional) return null;
  const result = value.trim().replace(/^0x/, "");
  if (!/^[0-9a-fA-F]{64}$/.test(result) || /^0+$/.test(result))
    throw new Error(
      "Use a nonzero 32-byte reference (64 hexadecimal characters).",
    );
  return result.toLowerCase();
}
export function nativeAmount(value: string): string {
  if (!/^\d+(\.\d{1,24})?$/.test(value))
    throw new Error(
      "Enter a valid NEAR amount with at most 24 decimal places.",
    );
  const [whole, fractional = ""] = value.split(".");
  return (
    BigInt(whole) * 10n ** 24n +
    BigInt(fractional.padEnd(24, "0"))
  ).toString();
}

export function slippageBasisPoints(value: string): number {
  if (!/^\d+(\.\d{1,2})?$/.test(value.trim()))
    throw new Error(
      "Slippage must be between 0 and 10%, with at most two decimals.",
    );
  const [whole, fraction = ""] = value.trim().split(".");
  const points = BigInt(whole) * 100n + BigInt(fraction.padEnd(2, "0"));
  if (points > 1_000n)
    throw new Error(
      "Slippage must be between 0 and 10%, with at most two decimals.",
    );
  return Number(points);
}
export function displayNative(value: string, decimals = 24): string {
  const amount = BigInt(value);
  const scale = 10n ** BigInt(decimals);
  return `${amount / scale}.${(amount % scale).toString().padStart(decimals, "0").replace(/0+$/, "") || "0"}`;
}
function randomReference(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

export interface ActionFieldsValue {
  chain: Chain;
  actor: string;
  amount: string;
  purpose: string;
  report: string;
  existingReference: boolean;
  manager: string;
  managerProfile: ProfileLookup | null;
  attempts: string;
  slippage: string;
  minutes: string;
  storage: string;
  reason: string;
  unverified: string;
  tokenAccount: string;
}
export type ChangeField = <K extends keyof ActionFieldsValue>(
  field: K,
  value: ActionFieldsValue[K],
) => void;

export function buildLocalAction(
  target: ActionTarget,
  values: ActionFieldsValue,
  parsedAmount: string,
  purposeRef: string | null,
  reportRef: string | null,
): LocalAction {
  const { kind } = target;
  const { chain } = values;
  const task = target.task?.id ?? "";
  switch (kind) {
    case "create_task":
      return { action: kind, amount: parsedAmount, purpose: purposeRef! };
    case "request_budget":
      return {
        action: kind,
        task,
        amount: parsedAmount,
        purpose: purposeRef!,
      };
    case "withdraw":
    case "return_funds":
    case "release":
      return { action: kind, task, amount: parsedAmount };
    case "finalize":
      return { action: kind, task, report: reportRef };
    case "fund":
      return {
        action: kind,
        amount: parsedAmount,
        invoice: target.invoice ? target.invoice.id : null,
        funding_reference: chain === "ethereum" ? null : randomReference(),
      };
    default:
      throw new Error("This is not a local wallet action.");
  }
}
export function buildCoordinatorAction(
  target: ActionTarget,
  values: ActionFieldsValue,
  parsedAmount: string,
  expires: number,
  purposeRef: string | null,
  reportRef: string | null,
): CoordinatorAction {
  const { kind } = target;
  const { chain, manager, attempts, reason, unverified } = values;
  const task = target.task?.id ?? "";
  const plan = target.plan?.id ?? "";
  switch (kind) {
    case "set_authority":
      if (!manager.trim())
        throw new Error(
          "Choose a manager’s verified NEAR wallet or enter their NEAR account.",
        );
      return { action: kind, account: manager.trim() };
    case "set_retry_limit":
      if (
        !/^\d+$/.test(attempts) ||
        Number(attempts) < 1 ||
        Number(attempts) > 20
      )
        throw new Error("Choose a recovery limit between 1 and 20 attempts.");
      return { action: kind, attempts: Number(attempts) };
    case "approve":
      return { action: kind, chain, request: target.request!.id };
    case "reject":
      return { action: kind, chain, request: target.request!.id, expires };
    case "pause":
    case "continue":
    case "cancel":
      return {
        action: "control",
        chain,
        task,
        control: { action: kind },
        expires,
      };
    case "reclaim":
      return {
        action: "control",
        chain,
        task,
        control: { action: kind, amount: parsedAmount },
        expires,
      };
    case "restore":
      return {
        action: kind,
        chain,
        task,
        amount: parsedAmount,
        purpose: purposeRef!,
        expires,
      };
    case "out_of_budget":
      return { action: kind, chain, task, expires };
    case "withdraw_free":
      return { action: kind, chain, amount: parsedAmount, expires };
    case "abort":
    case "requeue":
    case "resume_plan":
      return { action: kind, plan };
    case "retry":
      if (
        [reason, unverified].some(
          (value) => new TextEncoder().encode(value.trim()).length > 512,
        )
      )
        throw new Error(
          "Shorten each recovery explanation to at most 512 UTF-8 bytes.",
        );
      return {
        action: kind,
        plan,
        error: reason.trim() || null,
        unverified: unverified.trim() || null,
      };
    case "invalidate":
      return { action: kind, operation: target.command!.id, expires };
    case "open_invoice":
      return { action: kind, amount: parsedAmount, purpose: purposeRef! };
    case "respond_invoice":
      return {
        action: kind,
        invoice: target.invoice!.id,
        intended: parsedAmount,
        reference: reportRef,
      };
    default:
      throw new Error("This is not a coordinator action.");
  }
}
