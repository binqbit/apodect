/** Browser contract. Token amounts are USDC base-unit decimal strings. */
export type Chain = "ethereum" | "solana" | "near";
export type Amount = string;
export interface Wallet {
  chain: Chain;
  network: string;
  address: string;
}
export interface Profile {
  id: string;
  display_name: string | null;
}
export interface Account {
  session: { profile: Profile; wallet: Wallet; expires_at: string };
  wallets: Wallet[];
}
export interface Challenge {
  id: string;
  chain: Chain;
  address: string;
  message: string;
  nonce: number[];
  recipient: string;
  callback_url?: string;
  expires_at: string;
}
export interface SignedChallenge {
  challenge: string;
  signature: string;
  public_key?: string;
}
export interface PublicConfig {
  origin: string;
  local_simulator: boolean;
  wallet_networks: Record<Chain, string>;
  ethereum: { chain_id: string; genesis_hash: string };
  solana: { genesis_hash: string };
  near: {
    network_id: string;
    genesis: {
      chain_id: string;
      genesis_time: string;
      genesis_height: number;
      validators: unknown;
    };
  };
}
export interface ProjectRoles {
  owner: boolean;
  authority: boolean;
  agent: boolean;
  owner_actor: string | null;
  authority_actor: string | null;
  local_owners: Partial<Record<Chain, string>>;
}
export interface ProjectSummary {
  key: string;
  coordinator: string;
  project_id: string;
  indexed: boolean;
  observed_at: string | null;
  roles: ProjectRoles;
  available: Amount;
  held: Amount;
  pending_reserves: Amount;
  task_count: number;
  attention_count: number;
}
export interface ChainView {
  chain: Chain;
  vault: string;
  asset: string;
  owner_recipient: string | null;
  available: Amount;
  free: Amount;
  held: Amount;
  pending_reserves: Amount;
  local_free: Amount | null;
  observed_at: string | null;
  height: string | null;
  block_hash: string | null;
}
export interface TaskView {
  id: string;
  chain: Chain;
  agent: string;
  mine: boolean;
  state: "active" | "stopped" | "out_of_budget" | "completed" | "canceled";
  paused: boolean;
  started: boolean;
  available: Amount;
  issued: Amount;
  recovered: Amount;
  pending_outflow: Amount;
  live_request: string | null;
  report: string | null;
}
export interface RequestView {
  id: string;
  chain: Chain;
  task: string;
  amount: Amount;
  purpose: string;
  kind: "initial" | "supplement" | "resume" | "restoration";
  local_state: "pending" | "queued" | "funded" | "rejected" | "removed";
  status: string;
  approval_position: number | null;
  plan: string | null;
  queue_head: boolean;
}
export interface RouteView {
  id: string;
  source: Chain;
  input: Amount;
  min_input: Amount;
  output: Amount;
  received: Amount;
  returned: Amount;
  dispatched: Amount;
  unsettled_input: Amount;
  confirmed_transport_expense: Amount | null;
  expires: string;
}
export interface PlanView {
  id: string;
  destination: Chain;
  principal: Amount;
  local_amount: Amount;
  expires: string;
  status: string;
  attempts: number;
  requires_attention: boolean;
  error: string | null;
  queue_released: boolean;
  routes: RouteView[];
}
export interface InvoiceView {
  id: string;
  amount: Amount;
  purpose: string;
  intended: Amount | null;
  closed: boolean;
  response_reference: string | null;
  actual_funding: Amount;
}
export interface OperationView {
  id: string;
  chain: Chain;
  state: string;
  attempt: number;
  error: string | null;
  transaction_id: string | null;
}
export interface CommandView {
  id: string;
  chain: Chain;
  status: string;
  action: string;
  plan: string;
  task: string | null;
  request: string | null;
  amount: Amount;
  expires: string;
  signature_error: string | null;
}
export interface ProjectDetail extends ProjectSummary {
  owner: string;
  authority: string;
  retry_limit: number;
  chains: ChainView[];
  tasks: TaskView[];
  requests: RequestView[];
  plans: PlanView[];
  invoices: InvoiceView[];
  operations: OperationView[];
  commands: CommandView[];
}
export type LocalAction =
  | { action: "create_task"; amount: Amount; purpose: string }
  | { action: "request_budget"; task: string; amount: Amount; purpose: string }
  | {
      action: "withdraw" | "return_funds" | "release";
      task: string;
      amount: Amount;
    }
  | { action: "finalize"; task: string; report?: string | null }
  | {
      action: "fund";
      amount: Amount;
      invoice?: string | null;
      funding_reference?: string | null;
    };
export type TaskControl =
  | { action: "pause" | "continue" | "cancel" }
  | { action: "reclaim"; amount: Amount };
export type CoordinatorAction =
  | { action: "set_authority"; account: string }
  | { action: "set_retry_limit"; attempts: number }
  | { action: "approve"; chain: Chain; request: string }
  | { action: "reject"; chain: Chain; request: string; expires: number }
  | {
      action: "control";
      chain: Chain;
      task: string;
      control: TaskControl;
      expires: number;
    }
  | {
      action: "restore";
      chain: Chain;
      task: string;
      amount: Amount;
      purpose: string;
      expires: number;
    }
  | { action: "out_of_budget"; chain: Chain; task: string; expires: number }
  | { action: "withdraw_free"; chain: Chain; amount: Amount; expires: number }
  | { action: "abort" | "requeue" | "resume_plan"; plan: string }
  | {
      action: "retry";
      plan: string;
      error?: string | null;
      unverified?: string | null;
    }
  | { action: "invalidate"; operation: string; expires: number }
  | { action: "open_invoice"; amount: Amount; purpose: string }
  | {
      action: "respond_invoice";
      invoice: string;
      intended: Amount;
      reference?: string | null;
    };
export type UnsignedIntent =
  | {
      chain: "ethereum";
      chain_id: number;
      actor: string;
      transactions: { to: string; data: string; value: string }[];
    }
  | {
      chain: "solana";
      genesis_hash: string;
      actor: string;
      token_account_creation?: { address: string; rent_lamports: string };
      instructions: {
        program_id: string;
        accounts: { address: string; signer: boolean; writable: boolean }[];
        data_base64: string;
      }[];
    }
  | {
      chain: "near";
      network: {
        chain_id: string;
        genesis_time: string;
        genesis_height: number;
        validators: unknown;
      };
      actor: string;
      transactions: {
        receiver_id: string;
        method_name: string;
        args_base64: string;
        gas: number;
        deposit: string;
      }[];
    };
export interface FundingCosts {
  principal: Amount;
  local_amount: Amount;
  expected_transport_cost: Amount;
  maximum_transport_cost: Amount;
  warnings: { route: string; fee: Amount; threshold_bps: number }[];
}
export type ApplyPreparation =
  | { status: "waiting"; shortfall: Amount }
  | { status: "ready"; intent: UnsignedIntent; costs: FundingCosts };
export interface LocalPreparation {
  actor: string;
  chain: Chain;
  action: LocalAction;
  storage_deposit?: string;
  token_account?: string;
}
export interface CoordinatorPreparation {
  actor: string;
  action: CoordinatorAction;
  storage_deposit: string;
}
export interface ApplyRequest {
  actor: string;
  request: {
    destination: Chain;
    request: string;
    slippage_bps: number;
    expires: number;
  };
}
export interface ContentReference {
  digest: string;
  content: string;
}
export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export const sessionEvents = new EventTarget();

async function request<T>(
  path: string,
  method = "GET",
  body?: unknown,
): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    credentials: "same-origin",
    signal: AbortSignal.timeout(35_000),
    headers:
      body === undefined ? undefined : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    const error = await response.json().catch(() => ({
      error: {
        code: "unavailable",
        message: "The service is temporarily unavailable.",
      },
    }));
    const failure = new ApiError(
      response.status,
      error.error?.code ?? "request_failed",
      error.error?.message ?? "The request could not be completed.",
    );
    if (failure.status === 401 && failure.code === "authentication_required")
      sessionEvents.dispatchEvent(new Event("expired"));
    throw failure;
  }
  return response.status === 204
    ? (undefined as T)
    : (response.json() as Promise<T>);
}
const projectPath = (key: string) => `/projects/${encodeURIComponent(key)}`;
export const api = {
  config: () => request<PublicConfig>("/config"),
  account: () => request<Account>("/account"),
  rename: (name: string | null) =>
    request<Profile>("/account", "PATCH", { name }),
  challenge: (chain: Chain, address: string, callback_url?: string) =>
    request<Challenge>("/auth/challenge", "POST", {
      chain,
      address,
      callback_url,
    }),
  login: (proof: SignedChallenge) =>
    request<Account>("/auth/verify", "POST", proof),
  logout: () => request<void>("/auth/logout", "POST", {}),
  linkChallenge: (chain: Chain, address: string, callback_url?: string) =>
    request<Challenge>("/account/wallets/challenge", "POST", {
      chain,
      address,
      callback_url,
    }),
  linkWallet: (proof: SignedChallenge) =>
    request<Account>("/account/wallets/verify", "POST", proof),
  projects: () => request<ProjectSummary[]>("/projects"),
  project: (key: string) => request<ProjectDetail>(projectPath(key)),
  prepareLocal: (key: string, input: LocalPreparation) =>
    request<UnsignedIntent>(`${projectPath(key)}/actions/local`, "POST", input),
  prepareCoordinator: (key: string, input: CoordinatorPreparation) =>
    request<UnsignedIntent>(
      `${projectPath(key)}/actions/coordinator`,
      "POST",
      input,
    ),
  prepareApply: (key: string, input: ApplyRequest) =>
    request<ApplyPreparation>(
      `${projectPath(key)}/actions/apply`,
      "POST",
      input,
    ),
  saveReference: (key: string, content: string) =>
    request<ContentReference>(`${projectPath(key)}/references`, "POST", {
      content,
    }),
  reference: (key: string, digest: string) =>
    request<ContentReference>(
      `${projectPath(key)}/references/${encodeURIComponent(digest)}`,
    ),
  rpc: async <T>(chain: Chain, method: string, params: unknown): Promise<T> => {
    const response = await request<{
      jsonrpc: "2.0";
      id: number;
      result?: T;
      error?: { code: number; message: string; data?: { kind: string } };
    }>(`/rpc/${chain}`, "POST", { jsonrpc: "2.0", id: 1, method, params });
    if (response.error)
      throw new ApiError(
        422,
        `rpc_${response.error.data?.kind ?? "rejected"}`,
        response.error.message,
      );
    if (!("result" in response))
      throw new ApiError(
        502,
        "invalid_rpc_response",
        "The chain returned an invalid RPC response.",
      );
    return response.result as T;
  },
  transportStatus: (key: string, route: string) =>
    request<unknown>(
      `${projectPath(key)}/routes/${encodeURIComponent(route)}/status`,
    ),
};
