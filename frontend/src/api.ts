export interface AccountBalance {
  quota_raw: string;
  used_quota_raw: string;
  fetched_at: string;
}

export interface KeySummary {
  id: string;
  name: string;
  masked: string;
}

export type SelectedKeySummary = KeySummary;

export interface ActiveAccount {
  revision: string;
  username: string;
  upstream_user_id: string;
  balance: AccountBalance;
  selected_key: SelectedKeySummary;
  activated_at: string;
}

export interface CandidateAccount {
  id: string;
  username: string;
  upstream_user_id: string;
  balance: AccountBalance;
  keys: KeySummary[];
  expires_at: string;
}

export type ErrorCode =
  | "invalid_credentials" | "upstream_challenge" | "upstream_session_expired"
  | "upstream_unavailable" | "upstream_unexpected_response" | "key_material_unavailable"
  | "persistence_failed" | "operation_in_progress" | "account_not_configured"
  | "stale_revision" | "candidate_expired";

export interface OperationError {
  code: ErrorCode;
  message: string;
}

export interface BackgroundOperation {
  id: string;
  kind: "login" | "activate" | "refresh";
  status: "running" | "succeeded" | "failed";
  phase: "authenticating" | "reading_account" | "listing_keys" | "reading_key" | "committing" | "done";
  error: OperationError | null;
}

export interface AccountState {
  active: ActiveAccount | null;
  candidate: CandidateAccount | null;
  operation: BackgroundOperation | null;
}

export interface RevealedKey {
  active_revision: string;
  key_id: string;
  key: string;
}

export const phases: Record<BackgroundOperation["phase"], string> = {
  authenticating: "正在登录",
  reading_account: "正在读取余额",
  listing_keys: "正在读取已有密钥",
  reading_key: "正在获取所选密钥",
  committing: "正在保存账号",
  done: "已完成",
};

export function errorText(error: OperationError): string {
  if (error.code === "upstream_challenge") return "上游仍要求验证，暂时无法完成登录";
  if (error.code === "candidate_expired") return "候选账号已过期，请重新登录";
  return error.message || "操作未完成，请重试";
}

export class ApiError extends Error {
  constructor(public status: number, public detail: OperationError) {
    super(errorText(detail));
  }
}

export async function request<T>(path: string, init: RequestInit): Promise<T> {
  const response = await fetch(path, { ...init, credentials: "same-origin", cache: "no-store" });
  if (!response.ok) {
    let detail: OperationError = { code: "upstream_unavailable", message: `请求未完成（${response.status}），请重试` };
    try {
      const data = await response.json() as { error?: OperationError };
      if (data.error) detail = data.error;
    } catch { /* Non-JSON error responses must not be displayed as raw HTML. */ }
    throw new ApiError(response.status, detail);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

export function failureText(error: unknown): string {
  return error instanceof ApiError ? error.message : "无法连接本地服务，请检查后重试";
}
