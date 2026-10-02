import { useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import {
  AlertCircle,
  Calendar,
  Check,
  CheckCircle2,
  ChevronRight,
  Eye,
  FileText,
  Key,
  KeyRound,
  LayoutDashboard,
  Loader2,
  LogOut,
  Menu,
  Network,
  RefreshCw,
  Settings,
  Shield,
  ShieldAlert,
  ShieldCheck,
  User,
  Users,
  Wallet,
  X,
} from "lucide-react";
import { AccountsPanel } from "./AccountsPanel";
import { RequestLogPanel } from "./RequestLogPanel";
import { useManager } from "./useManager";
import type { GlobalCheckinConfig } from "./accountsTypes";
import {
  formatQuotaUsd,
  formatRemainingUsd,
  formatShanghaiDateTime,
  mapAccountDtoToManaged,
  mapLogItemToEntry,
} from "./accountsTypes";
import { OverviewPanel } from "./OverviewPanel";
import { SettingsPanel } from "./SettingsPanel";
import { RevealDialog } from "./RevealDialog";

export type AppPage = "overview" | "accounts" | "logs" | "settings";

export function App() {
  const manager = useManager();
  const { session, state, busy, loadError } = manager;

  // Root login password state
  const [rootPassword, setRootPassword] = useState("");
  const [submittingRoot, setSubmittingRoot] = useState(false);
  const [rootError, setRootError] = useState<string | null>(null);

  // Responsive mobile menu drawer state
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);

  // Detail reveal modal state
  const [detailRevealOpen, setDetailRevealOpen] = useState(false);

  // 4-page hash navigation
  const [currentPage, setCurrentPage] = useState<AppPage>(() => {
    if (typeof window !== "undefined") {
      const hash = window.location.hash.replace("#", "");
      if (
        hash === "overview" ||
        hash === "accounts" ||
        hash === "logs" ||
        hash === "settings"
      ) {
        return hash as AppPage;
      }
    }
    return "overview";
  });

  // Synchronize hash changes (browser back/forward & initial load)
  useEffect(() => {
    function handleHashChange() {
      const hash = window.location.hash.replace("#", "");
      if (
        hash === "overview" ||
        hash === "accounts" ||
        hash === "logs" ||
        hash === "settings"
      ) {
        setCurrentPage(hash as AppPage);
        setMobileMenuOpen(false);
      } else {
        setCurrentPage("overview");
      }
    }

    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  const navigateTo = (page: AppPage) => {
    window.location.hash = `#${page}`;
    setCurrentPage(page);
    setMobileMenuOpen(false);
    manager.selectDetailAccount(null);
  };

  // Handle Root Unlock Login
  async function handleUnlockSubmit(e: FormEvent) {
    e.preventDefault();
    const root = rootPassword.trim();
    if (!root) return;

    setRootPassword("");
    setSubmittingRoot(true);
    setRootError(null);
    try {
      await manager.unlock(root);
    } catch (err: any) {
      setRootError(err?.message || "密钥不正确，请重试");
    } finally {
      setSubmittingRoot(false);
    }
  }

  // Handle Logout
  async function handleLogout() {
    await manager.logout();
    setRootPassword("");
    setRootError(null);
    setDetailRevealOpen(false);
  }

  // Selected account for detail view
  const selectedAccount = useMemo(() => {
    if (!manager.selectedAccountId) return null;
    return state.accounts.find(a => a.id === manager.selectedAccountId) || null;
  }, [state.accounts, manager.selectedAccountId]);

  // Clean managed accounts list
  const managedAccounts = useMemo(() => {
    const schedule = manager.globalCheckin?.schedule || [];
    return state.accounts.map(a => {
      const scheduleItem = schedule.find(s => s.account_id === a.id);
      return mapAccountDtoToManaged(
        a,
        state.route_account_id,
        scheduleItem,
        a.id === manager.selectedAccountId ? manager.accountCheckin : undefined
      );
    });
  }, [
    state.accounts,
    state.route_account_id,
    manager.globalCheckin?.schedule,
    manager.selectedAccountId,
    manager.accountCheckin,
  ]);

  // Candidate TTL expiration calculation
  const expired = useMemo(() => {
    if (manager.candidateExpired) return true;
    if (!state.candidate?.expires_at) return false;
    const exp = new Date(state.candidate.expires_at).getTime();
    return !Number.isNaN(exp) && exp < Date.now();
  }, [manager.candidateExpired, state.candidate?.expires_at]);

  // Global Checkin Config Memo
  const globalCheckinConfig: GlobalCheckinConfig = useMemo(() => {
    if (!manager.globalCheckin?.settings) {
      return {
        enabled: false,
        startTime: "09:00",
        intervalMinutes: 30,
        timezone: "Asia/Shanghai",
        resetTime: "08:00",
        cycleDate: manager.globalCheckin?.cycle_date,
      };
    }
    return {
      enabled: manager.globalCheckin.settings.enabled,
      startTime: manager.globalCheckin.settings.time,
      intervalMinutes: manager.globalCheckin.settings.interval_minutes,
      timezone: manager.globalCheckin.settings.timezone || "Asia/Shanghai",
      resetTime: manager.globalCheckin.settings.reset_time || "08:00",
      cycleDate: manager.globalCheckin.cycle_date,
    };
  }, [manager.globalCheckin]);

  // Loading Session
  if (session === "loading") {
    return (
      <div className="login-screen">
        <div className="card login-card text-center">
          <div className="login-loading-content">
            <Loader2 size={32} className="spin text-teal" />
            <p className="secondary mt-4">正在连接本地服务…</p>
          </div>
        </div>
      </div>
    );
  }

  // Locked Screen
  if (session === "locked") {
    return (
      <div className="login-screen">
        <div className="card login-card">
          <div className="login-header">
            <div className="login-brand">
              <ShieldCheck size={28} className="text-teal" />
              <h1 className="login-brand-title">AnyRouter 管理面板</h1>
            </div>
          </div>

          {rootError && (
            <div className="alert alert-error" role="alert">
              <span>{rootError}</span>
            </div>
          )}

          {manager.notice && (
            <div className="alert alert-warn" role="status" style={{ marginBottom: "1rem" }}>
              <div>{manager.notice}</div>
              {manager.notice.includes("服务端会话注销失败") && (
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  style={{ marginTop: "0.5rem" }}
                  onClick={() => manager.logout()}
                >
                  重试注销
                </button>
              )}
            </div>
          )}

          <form onSubmit={handleUnlockSubmit} className="stack">
            <div className="field">
              <label htmlFor="admin-root">密钥</label>
              <input
                id="admin-root"
                type="password"
                required
                autoFocus
                autoComplete="off"
                placeholder="输入本地 root 密钥"
                value={rootPassword}
                onChange={e => setRootPassword(e.target.value)}
                disabled={submittingRoot}
              />
            </div>

            <div className="actions">
              <button
                type="submit"
                className="btn btn-primary btn-block"
                disabled={submittingRoot || !rootPassword.trim()}
              >
                {submittingRoot ? "正在建立会话…" : "进入"}
              </button>
            </div>
          </form>
        </div>
      </div>
    );
  }

  // Page titles
  const pageTitles: Record<AppPage, string> = {
    overview: "首页概览",
    accounts: "账号列表",
    logs: "网关请求日志",
    settings: "系统设置",
  };
  const pageTitle = pageTitles[currentPage];

  // Retained request logs
  const mappedRequestLogs = (manager.logs || []).map(mapLogItemToEntry);

  return (
    <div className="app-shell">
      {/* Mobile Top Header */}
      <header className="mobile-header">
        <div className="mobile-header-brand">
          <ShieldCheck size={20} className="text-teal" />
          <span className="font-semibold text-sm">AnyRouter</span>
        </div>
        <button
          type="button"
          className="btn btn-ghost btn-icon mobile-menu-btn"
          onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
          aria-label={mobileMenuOpen ? "关闭菜单" : "打开菜单"}
          aria-expanded={mobileMenuOpen}
        >
          {mobileMenuOpen ? <X size={20} /> : <Menu size={20} />}
        </button>
      </header>

      {/* Desktop Fixed Left Sidebar */}
      <aside className={`app-sidebar ${mobileMenuOpen ? "mobile-open" : ""}`}>
        <div className="sidebar-brand">
          <div className="brand-logo-wrap">
            <ShieldCheck size={24} className="text-teal" />
          </div>
          <div className="brand-text">
            <span className="brand-name font-semibold">AnyRouter</span>
            <span className="brand-sub secondary text-xs">本地管理</span>
          </div>
        </div>

        <nav className="sidebar-nav" aria-label="主要导航">
          <a
            href="#overview"
            className={`nav-link ${currentPage === "overview" ? "active" : ""}`}
            aria-current={currentPage === "overview" ? "page" : undefined}
            onClick={e => {
              e.preventDefault();
              navigateTo("overview");
            }}
          >
            <LayoutDashboard size={18} />
            <span>首页概览</span>
          </a>

          <a
            href="#accounts"
            className={`nav-link ${currentPage === "accounts" ? "active" : ""}`}
            aria-current={currentPage === "accounts" ? "page" : undefined}
            onClick={e => {
              e.preventDefault();
              navigateTo("accounts");
            }}
          >
            <Users size={18} />
            <span>账号列表</span>
            {managedAccounts.length > 0 && (
              <span className="nav-badge numeric">{managedAccounts.length}</span>
            )}
          </a>

          <a
            href="#logs"
            className={`nav-link ${currentPage === "logs" ? "active" : ""}`}
            aria-current={currentPage === "logs" ? "page" : undefined}
            onClick={e => {
              e.preventDefault();
              navigateTo("logs");
            }}
          >
            <FileText size={18} />
            <span>网关请求日志</span>
          </a>

          <a
            href="#settings"
            className={`nav-link ${currentPage === "settings" ? "active" : ""}`}
            aria-current={currentPage === "settings" ? "page" : undefined}
            onClick={e => {
              e.preventDefault();
              navigateTo("settings");
            }}
          >
            <Settings size={18} />
            <span>设置</span>
          </a>
        </nav>

        {/* Sidebar Footer with Logout Button */}
        <div className="sidebar-footer">
          <button
            type="button"
            className="btn btn-ghost btn-sm btn-block logout-btn"
            onClick={handleLogout}
            title="退出管理会话"
          >
            <LogOut size={16} />
            <span>退出管理</span>
          </button>
        </div>
      </aside>

      {/* Main Content Area */}
      <div className="app-main-content">
        <div className="content-header">
          <div>
            <h1 className="content-title">{pageTitle}</h1>
          </div>

          <div className="content-header-actions">
            {manager.loadError && (
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => void manager.load()}
              >
                <RefreshCw size={14} />
                <span>重试读取状态</span>
              </button>
            )}
          </div>
        </div>

        {/* Page Content Rendered by Hash Route */}
        <main className="page-body">
          {currentPage === "overview" && (
            <OverviewPanel
              accounts={managedAccounts}
              routedAccountId={state.route_account_id}
              pendingRouteAccountId={manager.pendingRouteAccountId}
              globalCheckinConfig={globalCheckinConfig}
              globalCheckinSchedule={manager.globalCheckin?.schedule || []}
              logs={mappedRequestLogs}
              logsLoading={manager.logsLoading}
              onNavigate={page => navigateTo(page)}
              onSelectRoute={id => manager.routeAccount(id)}
              onViewDetails={id => {
                manager.selectDetailAccount(id);
                navigateTo("accounts");
              }}
            />
          )}

          {currentPage === "accounts" && (
            <div className="stack">
              <AccountsPanel
                accounts={managedAccounts}
                routedAccountId={state.route_account_id}
                pendingRouteAccountId={manager.pendingRouteAccountId}
                globalCheckinConfig={globalCheckinConfig}
                busy={busy}
                hideGlobalCheckin={true}
                candidate={state.candidate}
                candidateExpired={expired}
                onSaveCandidate={async (candidateId, keyId) => {
                  await manager.saveCandidate(candidateId, keyId);
                }}
                operation={state.operation}
                onSelectRoute={id => manager.routeAccount(id)}
                onManualCheckin={(id, opts) => manager.runAccountCheckin(id, opts)}
                onAddAccount={creds => manager.addAccount(creds)}
                onSelectKey={(id, keyId) => manager.selectAccountKey(id, keyId)}
                onRefreshBalance={id => manager.refreshAccount(id)}
                onRevealKey={(id, rootKey, signal) => manager.revealKey(id, rootKey, signal)}
                onViewDetails={id =>
                  manager.selectDetailAccount(manager.selectedAccountId === id ? null : id)
                }
                selectedDetailAccountId={manager.selectedAccountId}
                onNavigateToSettings={() => navigateTo("settings")}
                routeError={manager.notice && manager.notice.includes("路由") ? manager.notice : null}
                error={manager.notice || null}
                requestLogs={mappedRequestLogs}
                refreshingAccountId={manager.refreshingAccountId}
                accountRefreshError={manager.accountRefreshError}
                accountCheckin={manager.accountCheckin}
                quotaAutoRefreshEnabled={manager.quotaAutoRefreshEnabled}
                quotaRefreshIntervalMinutes={manager.quotaRefreshIntervalMinutes}
                onToggleQuotaAutoRefresh={manager.setQuotaAutoRefreshEnabled}
                onChangeQuotaRefreshInterval={manager.setQuotaRefreshIntervalMinutes}
              />

              {/* Comprehensive Account Detail View */}
              {selectedAccount && (
                <section
                  className="card account-detail-panel stack"
                  aria-labelledby="account-detail-title"
                >
                  <div className="section-heading">
                    <div>
                      <h2 id="account-detail-title" className="text-lg font-semibold flex items-center gap-2">
                        <span>账号签到详情 · {selectedAccount.username}</span>
                        {selectedAccount.id === state.route_account_id && (
                          <span className="badge badge-success text-xs font-medium">当前路由</span>
                        )}
                      </h2>
                      <span className="text-secondary text-xs font-mono">
                        上游用户 ID：{selectedAccount.upstream_user_id} · 当前周期：
                        {manager.accountCheckin?.cycle_date || "08:00 日切"}
                      </span>
                    </div>
                    <button
                      type="button"
                      className="btn btn-secondary btn-sm"
                      onClick={() => manager.selectDetailAccount(null)}
                    >
                      关闭详情
                    </button>
                  </div>

                  {/* Quota & Balance Detail */}
                  <div className="account-subcard stack compact">
                    <div className="subcard-header flex items-center justify-between">
                      <span className="subcard-label flex items-center gap-1.5 text-xs font-semibold text-secondary">
                        <Wallet size={14} />
                        <span>额度字段与刷新状态</span>
                      </span>
                      <span className="text-xs text-secondary font-mono">
                        刷新时间：{selectedAccount.balance?.fetched_at ? formatShanghaiDateTime(selectedAccount.balance.fetched_at) : "尚未刷新"}
                      </span>
                    </div>
                    <div className="quota-detail-grid">
                      <div className="card quota-detail-item">
                        <span className="text-secondary text-xs">剩余额度</span>
                        <div className="flex items-baseline gap-2">
                          <span className="text-base font-bold font-mono text-teal tabular-nums">
                            {formatRemainingUsd(selectedAccount.balance?.quota_raw, selectedAccount.balance?.used_quota_raw)}
                          </span>
                          <span className="text-xs text-secondary font-mono">
                            (原始 raw: {selectedAccount.balance?.quota_raw || "—"})
                          </span>
                        </div>
                      </div>
                      <div className="card quota-detail-item">
                        <span className="text-secondary text-xs">已用额度</span>
                        <div className="flex items-baseline gap-2">
                          <span className="text-base font-bold font-mono text-secondary tabular-nums">
                            {formatQuotaUsd(selectedAccount.balance?.used_quota_raw)}
                          </span>
                          <span className="text-xs text-secondary font-mono">
                            (原始 raw: {selectedAccount.balance?.used_quota_raw || "—"})
                          </span>
                        </div>
                      </div>
                    </div>
                    <div className="text-xs text-secondary flex items-center justify-between">
                      <span>换算说明：按当前 500,000 raw / USD 参考计算，具体核算口径待上游确认。</span>
                    </div>
                  </div>

                  {/* Account Action Bar: Key Selection, Reveal & Route */}
                  <div className="account-subcard stack compact">
                    <div className="subcard-header">
                      <span className="subcard-label flex items-center gap-1.5">
                        <Key size={14} className="text-teal" />
                        <span>密钥与路由配置</span>
                      </span>
                    </div>

                    <div className="flex gap-4 flex-wrap items-end" style={{ marginTop: "0.5rem" }}>
                      <div className="field" style={{ minWidth: "220px", flex: 1 }}>
                        <label className="field-label text-xs">关联密钥选择</label>
                        <select
                          className="select-input"
                          value={selectedAccount.selected_key?.id || ""}
                          onChange={e => manager.selectAccountKey(selectedAccount.id, e.target.value)}
                          disabled={busy}
                          aria-label="关联密钥手选"
                        >
                          <option value="">不使用密钥（仅用于管理/签到）</option>
                          {selectedAccount.keys.map(k => (
                            <option key={k.id} value={k.id}>
                              {k.name || "未命名"} ({k.masked})
                            </option>
                          ))}
                        </select>
                      </div>

                      <div className="flex gap-2">
                        <button
                          type="button"
                          className="btn btn-secondary btn-sm"
                          onClick={() => setDetailRevealOpen(true)}
                          disabled={busy || !selectedAccount.selected_key}
                        >
                          <Eye size={14} />
                          <span>查看完整密钥</span>
                        </button>

                        <button
                          type="button"
                          className={`btn btn-sm ${
                            selectedAccount.id === state.route_account_id
                              ? "btn-route-active badge-success"
                              : "btn-primary"
                          }`}
                          disabled={
                            busy ||
                            selectedAccount.id === state.route_account_id ||
                            !selectedAccount.selected_key ||
                            selectedAccount.id === manager.pendingRouteAccountId
                          }
                          onClick={() => manager.routeAccount(selectedAccount.id)}
                        >
                          {selectedAccount.id === state.route_account_id ? (
                            <>
                              <Check size={14} />
                              <span>当前正在使用此账号路由</span>
                            </>
                          ) : selectedAccount.id === manager.pendingRouteAccountId ? (
                            <>
                              <Loader2 size={14} className="spin" />
                              <span>正在切换…</span>
                            </>
                          ) : !selectedAccount.selected_key ? (
                            <span>未选择密钥（无法路由）</span>
                          ) : (
                            <span>使用此账号作为网关路由</span>
                          )}
                        </button>
                      </div>
                    </div>
                  </div>

                  {/* Account Recent Request Logs (Filtered from retained logs) */}
                  <div className="account-subcard stack compact">
                    <div className="subcard-header">
                      <span className="subcard-label flex items-center gap-1.5">
                        <FileText size={14} className="text-secondary" />
                        <span>该账号本地网关调用记录</span>
                      </span>
                    </div>

                    {(() => {
                      const accountLogs = mappedRequestLogs.filter(
                        l => l.accountId === selectedAccount.id || l.accountName === selectedAccount.username
                      );
                      if (accountLogs.length === 0) {
                        return (
                          <p className="secondary small text-center p-3">
                            暂无此账号的网关调用记录
                          </p>
                        );
                      }
                      return (
                        <div className="log-table-wrapper table-responsive" style={{ maxHeight: "180px", overflowY: "auto" }}>
                          <table className="log-table data-table" aria-label="账号专属请求记录">
                            <thead>
                              <tr>
                                <th scope="col">时间</th>
                                <th scope="col">状态码</th>
                                <th scope="col">错误信息</th>
                              </tr>
                            </thead>
                            <tbody>
                              {accountLogs.slice(0, 10).map(l => (
                                <tr key={l.id}>
                                  <td className="text-xs font-mono">{formatShanghaiDateTime(l.timestamp)}</td>
                                  <td>
                                    {l.httpStatus !== null ? (
                                      <span className={`badge ${l.httpStatus < 300 ? "badge-success" : "badge-error"} text-xs font-mono`}>
                                        {l.httpStatus}
                                      </span>
                                    ) : (
                                      <span className="badge badge-warn text-xs">未收到响应</span>
                                    )}
                                  </td>
                                  <td className="text-xs secondary truncate" style={{ maxWidth: "240px" }}>
                                    {l.errorBody || "—"}
                                  </td>
                                </tr>
                              ))}
                            </tbody>
                          </table>
                        </div>
                      );
                    })()}
                  </div>

                  {/* Checkin Today & History */}
                  {manager.accountCheckin?.today && (
                    <div className="account-subcard stack compact">
                      <div className="subcard-header">
                        <span className="subcard-label flex items-center gap-1.5">
                          <Calendar size={14} className="text-secondary" />
                          <span>今日签到状态</span>
                        </span>
                        <span
                          className={`badge ${
                            manager.accountCheckin.today.status === "success" ||
                            manager.accountCheckin.today.status === "already_done"
                              ? "badge-success"
                              : manager.accountCheckin.today.status === "failed"
                              ? "badge-error"
                              : "badge-warn"
                          }`}
                        >
                          {manager.accountCheckin.today.status === "success"
                            ? "已签到"
                            : manager.accountCheckin.today.status === "already_done"
                            ? "今日已完成"
                            : manager.accountCheckin.today.status === "failed"
                            ? "签到失败"
                            : "状态未知"}
                        </span>
                      </div>
                      <div className="subcard-meta flex gap-4 text-xs secondary font-mono">
                        <span>
                          执行时间：{formatShanghaiDateTime(manager.accountCheckin.today.started_at)}
                        </span>
                        <span>
                          返回代码：{manager.accountCheckin.today.code || "无"}
                        </span>
                      </div>
                    </div>
                  )}

                  {/* History Table */}
                  <div className="account-subcard stack compact">
                    <div className="subcard-header">
                      <span className="subcard-label">历史签到记录</span>
                    </div>

                    <div className="checkin-table-wrapper table-responsive">
                      <table className="checkin-table data-table" aria-label="账号签到历史">
                        <thead>
                          <tr>
                            <th scope="col">日期</th>
                            <th scope="col">触发方式</th>
                            <th scope="col">状态</th>
                            <th scope="col">开始时间</th>
                            <th scope="col">结束时间</th>
                            <th scope="col">状态码</th>
                          </tr>
                        </thead>
                        <tbody>
                          {(manager.accountCheckin?.history || []).length === 0 ? (
                            <tr>
                              <td colSpan={6} className="text-center secondary small">
                                暂无历史签到记录
                              </td>
                            </tr>
                          ) : (
                            (manager.accountCheckin?.history || []).map((item, idx) => (
                              <tr key={`${item.date}-${idx}`}>
                                <td className="text-xs font-mono tabular-nums">{item.date}</td>
                                <td>{item.trigger === "manual" ? "手动" : "定时"}</td>
                                <td>
                                  <span
                                    className={`badge ${
                                      item.status === "success" || item.status === "already_done"
                                        ? "badge-success"
                                        : item.status === "failed"
                                        ? "badge-error"
                                        : "badge-warn"
                                    }`}
                                  >
                                    {item.status === "success" || item.status === "already_done"
                                      ? "成功"
                                      : item.status === "failed"
                                      ? "失败"
                                      : "未知"}
                                  </span>
                                </td>
                                <td className="text-xs font-mono tabular-nums">
                                  {formatShanghaiDateTime(item.started_at)}
                                </td>
                                <td className="text-xs font-mono tabular-nums">
                                  {formatShanghaiDateTime(item.finished_at)}
                                </td>
                                <td className="text-xs font-mono tabular-nums">
                                  {item.code || "—"}
                                </td>
                              </tr>
                            ))
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>

                  {/* Detail Reveal Dialog */}
                  {detailRevealOpen && selectedAccount && (
                    <RevealDialog
                      account={selectedAccount}
                      onClose={() => setDetailRevealOpen(false)}
                      onReveal={async (id, rootKey, signal) => {
                        return await manager.revealKey(id, rootKey, signal);
                      }}
                    />
                  )}
                </section>
              )}
            </div>
          )}

          {currentPage === "logs" && (
            <RequestLogPanel
              logs={mappedRequestLogs}
              loading={manager.logsLoading}
              error={manager.logsError}
              onRefresh={() => manager.loadLogs()}
              systemLogs={manager.systemLogs}
              systemLogsLoading={manager.systemLogsLoading}
              systemLogsError={manager.systemLogsError}
              onRefreshSystemLogs={params => manager.loadSystemLogs(params)}
              onClearSystemLogs={() => manager.clearSystemLogs()}
              onClearRequestLogs={() => manager.clearRequestLogs()}
            />
          )}

          {currentPage === "settings" && (
            <SettingsPanel
              globalConfig={globalCheckinConfig}
              busy={busy}
              onSaveGlobalCheckin={async cfg => {
                await manager.updateGlobalCheckin(cfg);
              }}
              logSettings={manager.logSettings}
              onSaveLogSettings={async settings => {
                await manager.saveLogSettings(settings);
              }}
              onClearSystemLogs={() => manager.clearSystemLogs()}
              onClearRequestLogs={() => manager.clearRequestLogs()}
            />
          )}
        </main>
      </div>
    </div>
  );
}
