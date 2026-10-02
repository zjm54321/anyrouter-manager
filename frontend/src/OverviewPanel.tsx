import { useMemo } from "react";
import {
  ArrowUpRight,
  CalendarCheck,
  Check,
  ChevronRight,
  Loader2,
  Network,
  Users,
  Wallet,
} from "lucide-react";
import type {
  GlobalCheckinConfig,
  ManagedAccount,
  RequestLogEntry,
} from "./accountsTypes";
import {
  formatQuotaUsd,
  formatRemainingUsd,
  formatShanghaiDateTime,
} from "./accountsTypes";

interface OverviewPanelProps {
  accounts: ManagedAccount[];
  routedAccountId: string | null;
  pendingRouteAccountId?: string | null;
  globalCheckinConfig: GlobalCheckinConfig;
  globalCheckinSchedule?: Array<{ account_id: string; scheduled_at: string; status: string }>;
  logs: RequestLogEntry[];
  logsLoading?: boolean;
  onNavigate: (page: "overview" | "accounts" | "logs" | "settings") => void;
  onSelectRoute?: (accountId: string) => void;
  onViewDetails?: (accountId: string) => void;
}

export function OverviewPanel({
  accounts,
  routedAccountId,
  pendingRouteAccountId,
  globalCheckinConfig,
  globalCheckinSchedule = [],
  logs,
  onNavigate,
  onSelectRoute,
  onViewDetails,
}: OverviewPanelProps) {
  const routedAccount = useMemo(() => {
    return accounts.find(a => a.id === routedAccountId) || null;
  }, [accounts, routedAccountId]);

  // Checkin counts from schedule or accounts
  const checkinCounts = useMemo(() => {
    let completed = 0;
    let failed = 0;
    let running = 0;
    let pending = 0;
    let unknown = 0;

    const source =
      globalCheckinSchedule.length > 0
        ? globalCheckinSchedule.map(s => s.status)
        : accounts.map(a => a.checkin?.status || "pending");

    for (const status of source) {
      if (status === "success" || status === "already_done") {
        completed++;
      } else if (status === "failed") {
        failed++;
      } else if (status === "running") {
        running++;
      } else if (status === "unknown") {
        unknown++;
      } else {
        pending++;
      }
    }

    return { completed, failed, running, pending, unknown, total: source.length };
  }, [globalCheckinSchedule, accounts]);

  // Aggregate quota using exact BigInt math
  const aggregateQuota = useMemo(() => {
    let totalQuotaRawBig = 0n;
    let totalUsedQuotaRawBig = 0n;
    let latestFetchedAt: string | null = null;
    let hasAnyValidBalance = false;

    for (const acc of accounts) {
      if (acc.balance) {
        try {
          if (acc.balance.quotaRaw) {
            totalQuotaRawBig += BigInt(acc.balance.quotaRaw.split(".")[0]);
            hasAnyValidBalance = true;
          }
          if (acc.balance.usedQuotaRaw) {
            totalUsedQuotaRawBig += BigInt(acc.balance.usedQuotaRaw.split(".")[0]);
          }
        } catch {
          // ignore
        }
        if (acc.balance.fetchedAt) {
          if (!latestFetchedAt || acc.balance.fetchedAt > latestFetchedAt) {
            latestFetchedAt = acc.balance.fetchedAt;
          }
        }
      }
    }

    return {
      remainingUsd: hasAnyValidBalance
        ? formatRemainingUsd(totalQuotaRawBig.toString(), totalUsedQuotaRawBig.toString())
        : "—",
      usedUsd: hasAnyValidBalance
        ? formatQuotaUsd(totalUsedQuotaRawBig.toString())
        : "—",
      latestFetchedAt,
    };
  }, [accounts]);

  return (
    <div className="overview-container stack">
      {/* Metrics Row */}
      <div className="metrics-grid">
        {/* Metric 1: Total Accounts */}
        <div className="card metric-card">
          <div className="metric-header">
            <span className="metric-label flex items-center gap-1.5">
              <Users size={15} className="text-secondary" />
              <span>账号总数</span>
            </span>
            <button
              type="button"
              className="text-link small"
              onClick={() => onNavigate("accounts")}
            >
              <span>管理</span>
              <ArrowUpRight size={13} />
            </button>
          </div>
          <div className="metric-value-row">
            <span className="metric-value numeric">{accounts.length}</span>
            <span className="secondary small">个已绑定账号</span>
          </div>
        </div>

        {/* Metric 2: Active Routed Account */}
        <div className="card metric-card">
          <div className="metric-header">
            <span className="metric-label flex items-center gap-1.5">
              <Network size={15} className="text-teal" />
              <span>当前网关路由</span>
            </span>
            <button
              type="button"
              className="text-link small"
              onClick={() => onNavigate("accounts")}
            >
              <span>切换</span>
              <ArrowUpRight size={13} />
            </button>
          </div>
          <div className="metric-value-row">
            {routedAccount ? (
              <div>
                <div className="metric-highlight">
                  <span className="status-indicator online" aria-hidden="true" />
                  <strong className="text-primary">{routedAccount.username}</strong>
                </div>
                <div className="secondary small numeric mono-font" style={{ marginTop: "4px" }}>
                  {routedAccount.selectedKey
                    ? `Key: ${routedAccount.selectedKey.masked}`
                    : "无可用密钥（无法路由）"}
                </div>
              </div>
            ) : (
              <span className="secondary small">未指定路由账号</span>
            )}
          </div>
        </div>

        {/* Metric 3: Today's Checkin Cycle */}
        <div className="card metric-card">
          <div className="metric-header">
            <span className="metric-label flex items-center gap-1.5">
              <CalendarCheck size={15} className="text-secondary" />
              <span>今日签到周期</span>
            </span>
            <span className="secondary small numeric">
              {globalCheckinConfig.cycleDate ? `${globalCheckinConfig.cycleDate} 周期` : "08:00 日切"}
            </span>
          </div>
          <div className="checkin-stat-pills">
            <span className="stat-pill success" title="已完成">
              已完成 {checkinCounts.completed}
            </span>
            {checkinCounts.failed > 0 && (
              <span className="stat-pill error" title="失败">
                失败 {checkinCounts.failed}
              </span>
            )}
            {checkinCounts.unknown > 0 && (
              <span className="stat-pill warn" title="待确认">
                待确认 {checkinCounts.unknown}
              </span>
            )}
            <span className="stat-pill neutral" title="未执行/待执行">
              待执行 {checkinCounts.pending + checkinCounts.running}
            </span>
          </div>
        </div>

        {/* Metric 4: USD Quota Aggregate */}
        <div className="card metric-card">
          <div className="metric-header">
            <span className="metric-label flex items-center gap-1.5">
              <Wallet size={15} className="text-secondary" />
              <span>额度统计（换算参考）</span>
            </span>
            {aggregateQuota.latestFetchedAt && (
              <span className="secondary small numeric font-mono" title="最新刷新时间">
                {formatShanghaiDateTime(aggregateQuota.latestFetchedAt)}
              </span>
            )}
          </div>
          <div className="quota-summary-numbers numeric">
            <div>
              <span className="secondary small">剩余：</span>
              <strong className="text-teal font-mono">{aggregateQuota.remainingUsd}</strong>
            </div>
            <div>
              <span className="secondary small">已用：</span>
              <span className="font-mono text-secondary">{aggregateQuota.usedUsd}</span>
            </div>
          </div>
        </div>
      </div>

      {/* Full Accounts Table (reused from accounts list, replacing recent requests) */}
      <section className="card overview-accounts-section stack" aria-labelledby="overview-accounts-heading">
        <div className="section-heading">
          <div>
            <h3 id="overview-accounts-heading" className="text-base font-semibold">账号列表</h3>
          </div>
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            onClick={() => onNavigate("accounts")}
          >
            <span>进入账号管理 ({accounts.length})</span>
            <ChevronRight size={14} />
          </button>
        </div>

        {accounts.length === 0 ? (
          <div className="empty-state text-center p-6">
            <p className="secondary">尚未添加任何账号</p>
          </div>
        ) : (
          <div className="accounts-table-wrapper table-responsive">
            <table className="accounts-table data-table" aria-label="首页账号列表">
              <thead>
                <tr>
                  <th scope="col" className="col-account">名称</th>
                  <th scope="col" className="col-activity">调用情况</th>
                  <th scope="col" className="col-quota">剩余额度（参考）</th>
                  <th scope="col" className="col-quota">已用额度（参考）</th>
                  <th scope="col" className="col-time">更新时间</th>
                  <th scope="col" className="col-actions text-right">操作</th>
                </tr>
              </thead>
              <tbody>
                {accounts.map(account => {
                  const isRouted = account.id === routedAccountId;
                  const isPendingRoute = account.id === pendingRouteAccountId;
                  const hasKey = !!account.selectedKey;

                  // Local request stats from retained logs
                  const accountLogs = (logs || []).filter(
                    l => l.accountId === account.id || l.accountName === account.username
                  );
                  const totalCalls = accountLogs.length;
                  const successCalls = accountLogs.filter(
                    l => l.httpStatus !== null && l.httpStatus >= 200 && l.httpStatus < 300
                  ).length;

                  return (
                    <tr
                      key={account.id}
                      className={`account-row account-card cursor-pointer ${isRouted ? "is-routed-row is-routed-card" : ""}`}
                      onClick={() => {
                        if (onViewDetails) onViewDetails(account.id);
                        onNavigate("accounts");
                      }}
                    >
                      <td className="col-account">
                        <div className="account-cell-content">
                          <div className="account-title-row">
                            <span className="font-semibold text-primary">{account.username}</span>
                            {isRouted && (
                              <span className="badge badge-success text-xs font-medium">当前路由</span>
                            )}
                            {isPendingRoute && (
                              <span className="badge badge-warn text-xs font-medium inline-flex items-center gap-1">
                                <Loader2 size={10} className="spin" />
                                <span>切换中…</span>
                              </span>
                            )}
                          </div>
                        </div>
                      </td>
                      <td className="col-activity tabular-nums">
                        {totalCalls === 0 ? (
                          <span className="text-secondary text-xs">—</span>
                        ) : (
                          <span className="text-xs inline-flex items-center gap-1">
                            <span className={`status-dot ${totalCalls === successCalls ? "dot-success" : "dot-warn"}`} />
                            <span>{successCalls}/{totalCalls} 成功</span>
                          </span>
                        )}
                      </td>
                      <td className="col-quota tabular-nums">
                        <span className="font-semibold text-teal font-mono">
                          {formatRemainingUsd(account.balance.quotaRaw, account.balance.usedQuotaRaw)}
                        </span>
                      </td>
                      <td className="col-quota tabular-nums">
                        <span className="text-secondary font-mono">
                          {formatQuotaUsd(account.balance.usedQuotaRaw)}
                        </span>
                      </td>
                      <td className="col-time tabular-nums text-xs text-secondary font-mono">
                        {formatShanghaiDateTime(account.balance.fetchedAt)}
                      </td>
                      <td className="col-actions text-right" onClick={e => e.stopPropagation()}>
                        <div className="account-actions-cell">
                          <button
                            type="button"
                            className="icon-action-btn detail-toggle-btn"
                            onClick={() => {
                              if (onViewDetails) onViewDetails(account.id);
                              onNavigate("accounts");
                            }}
                            title="查看详情"
                            aria-label="查看详情"
                          >
                            <span className="text-xs">详情</span>
                            <ChevronRight size={14} className="chevron-icon" />
                          </button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
