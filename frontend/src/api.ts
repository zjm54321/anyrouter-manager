export interface AccountBalance {
  quota_raw: string;
  used_quota_raw: string;
  fetched_at: string;
}

export interface KeySummary {
  id: string;
  name: string;
  masked: string;
  enabled?: boolean;
}

export interface AccountDTO {
  id: string;
  revision: string;
  username: string;
  upstream_user_id: string;
  balance: AccountBalance;
  keys: KeySummary[];
  selected_key: KeySummary | null;
  added_at: string;
}

export interface CandidateDTO {
  id: string;
  username: string;
  upstream_user_id: string;
  balance: AccountBalance;
  keys: KeySummary[];
  expires_at: string;
}

export type ErrorCode =
  | "invalid_credentials"
  | "upstream_challenge"
  | "upstream_session_expired"
  | "upstream_unavailable"
  | "upstream_unexpected_response"
  | "key_material_unavailable"
  | "persistence_failed"
  | "operation_in_progress"
  | "account_not_configured"
  | "stale_revision"
  | "candidate_expired"
  | "candidate_not_found"
  | "account_not_found"
  | "account_has_no_valid_key"
  | "schedule_overflow"
  | "checkin_retry_confirmation_required";

export interface OperationError {
  code: ErrorCode | string;
  message: string;
}

export interface OperationDTO {
  id: string;
  account_id: string | null;
  kind: "login" | "save" | "refresh" | "select_key" | "checkin";
  status: "running" | "succeeded" | "failed";
  phase:
    | "authenticating"
    | "reading_account"
    | "listing_keys"
    | "reading_key"
    | "committing"
    | "checking_in"
    | "done"
    | string;
  error: OperationError | null;
}

export interface AccountsResponse {
  accounts: AccountDTO[];
  route_account_id: string | null;
  candidate: CandidateDTO | null;
  operation: OperationDTO | null;
}

export interface RevealedKeyResponse {
  account_id: string;
  revision: string;
  key_id: string;
  key: string;
}

export interface CheckinSettings {
  enabled: boolean;
  time: string;
  interval_minutes: number;
  timezone: "Asia/Shanghai";
  reset_time: "08:00";
}

export interface CheckinScheduleItem {
  account_id: string;
  scheduled_at: string;
  status: "pending" | "running" | "success" | "already_done" | "failed" | "unknown";
}

export interface GlobalCheckinResponse {
  settings: CheckinSettings;
  cycle_date: string;
  next_run_at: string | null;
  schedule: CheckinScheduleItem[];
}

export interface AccountCheckinRecord {
  date: string;
  account_user_id: string;
  trigger: "manual" | "scheduled";
  status: "running" | "success" | "already_done" | "failed" | "unknown";
  started_at: string;
  finished_at: string | null;
  code: string | null;
}

export interface AccountCheckinResponse {
  account_id: string;
  cycle_date: string;
  today: AccountCheckinRecord | null;
  history: AccountCheckinRecord[];
  next_run_at: string | null;
}

export interface RequestLogItem {
  id: string;
  timestamp: string;
  account_id: string | null;
  account_name: string | null;
  http_status: number | null;
  error_body: string | null;
  truncated: boolean;
}

export interface RequestLogsResponse {
  items: RequestLogItem[];
  dropped_count: number;
}

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export interface LogSettings {
  level: LogLevel;
  system_retention_days: number;
  request_retention_days: number;
}

export interface SystemEvent {
  id: string;
  timestamp: string;
  level: LogLevel;
  event: string;
  operation_id: string | null;
  account_id: string | null;
  stage: string | null;
  elapsed_ms: number | null;
  http_status: number | null;
  reason: string | null;
  diagnostics: string | null;
}

export interface SystemLogsResponse {
  items: SystemEvent[];
  dropped_count: number;
}

// Backward-compatibility aliases
export type AccountState = AccountsResponse;
export type ActiveAccount = AccountDTO;
export type CandidateAccount = CandidateDTO;
export type BackgroundOperation = OperationDTO;
export type CheckinFullStatus = GlobalCheckinResponse;
export type RevealedKey = RevealedKeyResponse;

export const phases: Record<string, string> = {
  authenticating: "正在登录",
  reading_account: "正在读取余额",
  listing_keys: "正在读取已有密钥",
  reading_key: "正在获取所选密钥",
  committing: "正在保存账号",
  checking_in: "正在签到",
  done: "已完成",
};

export function errorText(error: OperationError): string {
  if (error.code === "upstream_challenge") return "上游仍要求验证，暂时无法完成登录";
  if (error.code === "candidate_expired") return "候选账号已过期，请重新登录";
  if (error.code === "checkin_retry_confirmation_required")
    return "当前周期签到结果处于未确认状态，重试可能导致重复提交，请确认后重试";
  if (error.code === "account_has_no_valid_key")
    return "该账号未关联有效密钥，无法作为网关路由出口";
  if (error.code === "schedule_overflow")
    return "签到排期跨天溢出，请缩短间隔或调整起始时间";
  if (error.code === "stale_revision") return "账号已被其他操作修改，请刷新后重试";
  if (error.code === "account_not_found") return "找不到指定的账号，请刷新列表";
  return error.message || "操作未完成，请重试";
}

export class ApiError extends Error {
  constructor(public status: number, public detail: OperationError) {
    super(errorText(detail));
  }
}

export async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, { ...init, credentials: "same-origin", cache: "no-store" });
  if (!response || !response.ok) {
    if (response?.status === 401) {
      throw new ApiError(401, {
        code: "unauthorized",
        message: "管理请求未通过鉴权，请重试或退出后重新建立管理会话",
      });
    }
    let detail: OperationError = {
      code: "upstream_unavailable",
      message: `请求未完成（${response?.status || 500}），请重试`,
    };
    try {
      const data = (await response?.json()) as { error?: OperationError };
      if (data?.error) detail = data.error;
    } catch {
      /* Non-JSON error responses must not be displayed as raw HTML. */
    }
    throw new ApiError(response?.status || 500, detail);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

export function failureText(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof Error) return error.message;
  return "无法连接本地服务，请检查后重试";
}

// ----------------------------------------------------------------------------
// API Endpoints (All frozen relative URLs, no-store, same-origin credentials)
// ----------------------------------------------------------------------------

export async function fetchAccounts(signal?: AbortSignal): Promise<AccountsResponse> {
  return request<AccountsResponse>("/api/accounts", { signal });
}

export async function adminLogin(root: string, signal?: AbortSignal): Promise<void> {
  return request<void>("/api/admin/session", {
    method: "POST",
    headers: { Authorization: `Bearer ${root}` },
    signal,
  });
}

export async function adminLogout(signal?: AbortSignal): Promise<void> {
  return request<void>("/api/admin/session", {
    method: "DELETE",
    signal,
  });
}

export async function loginAccount(
  username: string,
  password: string,
  signal?: AbortSignal
): Promise<{ operation_id: string }> {
  return request<{ operation_id: string }>("/api/accounts/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ username, password }),
    signal,
  });
}

export async function saveAccount(
  candidate_id: string,
  key_id?: string | null,
  signal?: AbortSignal
): Promise<{ operation_id: string }> {
  return request<{ operation_id: string }>("/api/accounts/save", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ candidate_id, key_id: key_id ?? null }),
    signal,
  });
}

export async function refreshAccount(
  id: string,
  signal?: AbortSignal
): Promise<{ operation_id: string }> {
  return request<{ operation_id: string }>(`/api/accounts/${id}/refresh`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({}),
    signal,
  });
}

export async function selectAccountKey(
  id: string,
  revision: string,
  key_id: string,
  signal?: AbortSignal
): Promise<{ operation_id: string }> {
  return request<{ operation_id: string }>(`/api/accounts/${id}/key/select`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ revision, key_id }),
    signal,
  });
}

export async function routeAccount(
  id: string,
  revision: string,
  signal?: AbortSignal
): Promise<AccountsResponse> {
  return request<AccountsResponse>(`/api/accounts/${id}/route`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ revision }),
    signal,
  });
}

export async function revealAccountKey(
  id: string,
  revision: string,
  root: string,
  signal?: AbortSignal
): Promise<RevealedKeyResponse> {
  return request<RevealedKeyResponse>(`/api/accounts/${id}/key/reveal`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${root}`,
    },
    body: JSON.stringify({ revision }),
    signal,
  });
}

export async function fetchGlobalCheckin(signal?: AbortSignal): Promise<GlobalCheckinResponse> {
  return request<GlobalCheckinResponse>("/api/checkin", { signal });
}

export async function updateGlobalCheckinSettings(
  settings: { enabled: boolean; time: string; interval_minutes: number },
  signal?: AbortSignal
): Promise<GlobalCheckinResponse> {
  return request<GlobalCheckinResponse>("/api/checkin/settings", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(settings),
    signal,
  });
}

export async function fetchAccountCheckin(
  id: string,
  signal?: AbortSignal
): Promise<AccountCheckinResponse> {
  return request<AccountCheckinResponse>(`/api/accounts/${id}/checkin`, { signal });
}

export async function runAccountCheckin(
  id: string,
  confirm_retry = false,
  signal?: AbortSignal
): Promise<{ operation_id?: string; already_recorded?: boolean }> {
  return request<{ operation_id?: string; already_recorded?: boolean }>(
    `/api/accounts/${id}/checkin/run`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ confirm_retry }),
      signal,
    }
  );
}

export async function fetchRequestLogs(
  limit = 100,
  signal?: AbortSignal
): Promise<RequestLogsResponse> {
  try {
    return await request<RequestLogsResponse>(`/api/request-logs?limit=${limit}`, { signal });
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) {
      throw error;
    }
    return { items: [], dropped_count: 0 };
  }
}

export async function fetchLogSettings(signal?: AbortSignal): Promise<LogSettings> {
  return request<LogSettings>("/api/log-settings", { signal });
}

export async function updateLogSettings(
  settings: LogSettings,
  signal?: AbortSignal
): Promise<LogSettings> {
  return request<LogSettings>("/api/log-settings", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(settings),
    signal,
  });
}

export async function fetchSystemLogs(
  params?: { limit?: number; level?: string; account_id?: string; operation_id?: string },
  signal?: AbortSignal
): Promise<SystemLogsResponse> {
  const query = new URLSearchParams();
  if (params?.limit) query.set("limit", String(params.limit));
  if (params?.level) query.set("level", params.level);
  if (params?.account_id) query.set("account_id", params.account_id);
  if (params?.operation_id) query.set("operation_id", params.operation_id);
  const qStr = query.toString();
  const endpoint = `/api/system-logs${qStr ? `?${qStr}` : ""}`;
  try {
    return await request<SystemLogsResponse>(endpoint, { signal });
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) {
      throw error;
    }
    return { items: [], dropped_count: 0 };
  }
}

export async function clearSystemLogs(signal?: AbortSignal): Promise<void> {
  await request<void>("/api/system-logs/clear", {
    method: "POST",
    signal,
  });
}

export async function clearRequestLogs(signal?: AbortSignal): Promise<void> {
  await request<void>("/api/request-logs/clear", {
    method: "POST",
    signal,
  });
}
