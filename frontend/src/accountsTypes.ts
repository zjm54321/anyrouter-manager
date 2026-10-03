import type {
  AccountCheckinResponse,
  AccountDTO,
  CheckinScheduleItem,
  RequestLogItem,
} from "./api";

export interface AccountKeySummary {
  id: string;
  name: string;
  masked: string;
  enabled?: boolean;
}

export interface AccountBalanceInfo {
  quotaRaw: string;
  usedQuotaRaw: string;
  fetchedAt: string;
}

export type AccountCheckinStatus =
  | "success"
  | "already_done"
  | "failed"
  | "unknown"
  | "running"
  | "pending";

export interface AccountCheckinSummary {
  status: AccountCheckinStatus;
  date: string; // YYYY-MM-DD cycle date from server (08:00 CST boundary)
  finishedAt?: string | null;
  startedAt?: string | null;
  code?: string | null;
  message?: string | null;
  scheduledAt?: string | null; // server-provided per-account scheduled slot (e.g. "09:30")
}

export interface ManagedAccount {
  id: string;
  revision?: string;
  username: string;
  upstreamUserId: string;
  balance: AccountBalanceInfo;
  selectedKey: AccountKeySummary | null;
  keys: AccountKeySummary[];
  checkin: AccountCheckinSummary | null;
  isRouted: boolean;
  addedAt?: string;
}

export interface GlobalCheckinConfig {
  enabled: boolean;
  startTime: string; // HH:MM fixed first slot, default "09:00"
  intervalMinutes: number; // 1 - 1440, default 30
  timezone?: string;
  resetTime?: string;
  cycleDate?: string;
}

export interface AccountsPanelProps {
  accounts: ManagedAccount[];
  routedAccountId: string | null;
  pendingRouteAccountId?: string | null;
  globalCheckinConfig?: GlobalCheckinConfig;
  busy?: boolean;
  onSelectRoute?: (accountId: string) => Promise<void> | void;
  onManualCheckin?: (accountId: string, options: { confirmRetry: boolean }) => Promise<void> | void;
  onAddAccount?: (credentials: { username: string; password: string }) => Promise<void> | void;
  onSelectKey?: (accountId: string, keyId: string) => Promise<void> | void;
  onRefreshBalance?: (accountId: string) => Promise<void> | void;
  onRevealKey?: (accountId: string, rootKey: string, signal?: AbortSignal) => Promise<string> | string;
  onUpdateGlobalCheckin?: (config: {
    enabled: boolean;
    startTime: string;
    intervalMinutes: number;
  }) => Promise<void> | void;
  onViewDetails?: (accountId: string) => void;
  selectedDetailAccountId?: string | null;
  hideGlobalCheckin?: boolean;
  error?: string | null;
  requestLogs?: RequestLogEntry[];
  refreshingAccountId?: string | null;
  accountRefreshError?: Record<string, string>;
  quotaAutoRefreshEnabled?: boolean;
  quotaRefreshIntervalMinutes?: 5 | 10;
  onToggleQuotaAutoRefresh?: (enabled: boolean) => void;
  onChangeQuotaRefreshInterval?: (minutes: 5 | 10) => void;
}

export interface RequestLogEntry {
  id: string;
  timestamp: string; // ISO string
  accountId: string;
  accountName: string;
  httpStatus: number | null; // null = 未收到响应（不伪造 200）
  errorBody: string | null; // upstream error raw text, non-2xx only
  truncated?: boolean;
  forwardingMode?: "pass" | "adapt" | null;
}

export function mapAccountDtoToManaged(
  dto: AccountDTO,
  routeAccountId: string | null,
  scheduleItem?: CheckinScheduleItem | null,
  accountCheckin?: AccountCheckinResponse | null
): ManagedAccount {
  let checkinSummary: AccountCheckinSummary | null = null;
  if (accountCheckin?.today) {
    const today = accountCheckin.today;
    checkinSummary = {
      status: today.status as AccountCheckinStatus,
      date: today.date,
      finishedAt: today.finished_at,
      startedAt: today.started_at,
      code: today.code,
      scheduledAt: scheduleItem?.scheduled_at || null,
    };
  } else if (scheduleItem) {
    checkinSummary = {
      status: scheduleItem.status as AccountCheckinStatus,
      date: "",
      scheduledAt: scheduleItem.scheduled_at,
    };
  }

  return {
    id: dto.id,
    revision: dto.revision,
    username: dto.username,
    upstreamUserId: dto.upstream_user_id,
    balance: {
      quotaRaw: dto.balance.quota_raw,
      usedQuotaRaw: dto.balance.used_quota_raw,
      fetchedAt: dto.balance.fetched_at,
    },
    selectedKey: dto.selected_key
      ? {
          id: dto.selected_key.id,
          name: dto.selected_key.name,
          masked: dto.selected_key.masked,
          enabled: dto.selected_key.enabled,
        }
      : null,
    keys: dto.keys.map(k => ({
      id: k.id,
      name: k.name,
      masked: k.masked,
      enabled: k.enabled,
    })),
    checkin: checkinSummary,
    isRouted: dto.id === routeAccountId,
    addedAt: dto.added_at,
  };
}

export function mapLogItemToEntry(item: RequestLogItem): RequestLogEntry {
  return {
    id: item.id,
    timestamp: item.timestamp,
    accountId: item.account_id || "",
    accountName: item.account_name || "未指定账号",
    httpStatus: item.http_status,
    errorBody: item.error_body,
    truncated: item.truncated,
    forwardingMode: item.forwarding_mode ?? null,
  };
}

export function formatTimeInSlot(startTime: string, intervalMinutes: number, index: number): string {
  const [hStr, mStr] = startTime.split(":");
  const h = parseInt(hStr || "9", 10);
  const m = parseInt(mStr || "0", 10);
  const totalMinutes = h * 60 + m + index * intervalMinutes;
  const slotH = Math.floor(totalMinutes / 60) % 24;
  const slotM = totalMinutes % 60;
  return `${slotH.toString().padStart(2, "0")}:${slotM.toString().padStart(2, "0")}`;
}

export function formatShanghaiDateTime(iso: string | null | undefined): string {
  if (!iso) return "-";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "-";
  return new Intl.DateTimeFormat("zh-CN", {
    timeZone: "Asia/Shanghai",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date);
}

export function formatRawQuota(raw: string | number | null | undefined): string {
  if (raw === null || raw === undefined || raw === "") return "0";
  return String(raw);
}

export function formatCstDate(isoDate: string): string {
  if (!isoDate) return "—";
  return isoDate.split("T")[0] || isoDate;
}

/**
 * Converts raw quota to USD ($X.XX) using New API conversion policy: 500,000 raw per USD.
 * Exact BigInt half-up calculation: cents = (raw * 100n + 250000n) / 500000n.
 * Returns "—" for invalid/empty/negative inputs.
 */
export function formatQuotaUsd(raw: string | number | null | undefined): string {
  if (raw === null || raw === undefined) return "—";
  const str = String(raw).trim();
  if (!str) return "—";
  const intPart = str.split(".")[0];
  if (!/^-?\d+$/.test(intPart)) return "—";
  try {
    const rawVal = BigInt(intPart);
    if (rawVal < 0n) return "—";
    const cents = (rawVal * 100n + 250000n) / 500000n;
    const dollars = cents / 100n;
    const remCents = cents % 100n;
    return `$${dollars.toString()}.${remCents.toString().padStart(2, "0")}`;
  } catch {
    return "—";
  }
}
/**
 * Formats remaining wallet quota as reference USD amount ($X.XX) based on 500,000 raw per unit.
 * In upstream New API semantics, `quota` represents the current remaining wallet quota directly.
 * (used_quota is tracked independently, so deducting used from quota would double-subtract).
 * Note: 500,000 raw per USD factor is an unverified reference convention.
 * Returns "—" for invalid/empty/negative inputs.
 */
export function formatRemainingUsd(
  quotaRaw: string | number | null | undefined,
  _usedRaw?: string | number | null | undefined
): string {
  return formatQuotaUsd(quotaRaw);
}
