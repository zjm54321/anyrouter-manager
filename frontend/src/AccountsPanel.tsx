import { Fragment, useEffect, useId, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import {
  AlertCircle,
  AlertTriangle,
  Calendar,
  Check,
  CheckCircle2,
  ChevronRight,
  Clock,
  Copy,
  ExternalLink,
  Eye,
  FileText,
  HelpCircle,
  Key,
  KeyRound,
  Loader2,
  Plus,
  RefreshCw,
  Shield,
  ShieldCheck,
  UserCheck,
  UserPlus,
  Users,
  Wallet,
  X,
} from "lucide-react";
import type { AccountCheckinRecord, AccountCheckinResponse, CandidateDTO, OperationDTO } from "./api";
import { errorText } from "./api";
import type {
  AccountsPanelProps,
  ManagedAccount,
} from "./accountsTypes";
import {
  formatCstDate,
  formatQuotaUsd,
  formatRemainingUsd,
  formatRawQuota,
  formatShanghaiDateTime,
  formatTimeInSlot,
} from "./accountsTypes";
import { RevealDialog } from "./RevealDialog";
export { formatTimeInSlot, formatShanghaiDateTime };
import { GlobalCheckinCard } from "./GlobalCheckinCard";
import { AddAccountModal } from "./AddAccountModal";

const checkinStatusLabels: Record<string, string> = {
  success: "已签到",
  already_done: "今日已完成",
  failed: "签到失败",
  unknown: "状态未知（需确认）",
  running: "正在签到",
  pending: "待执行",
};

const phaseLabelsLocal: Record<string, string> = {
  submitted: "已提交",
  launching_browser: "正在启动浏览器",
  logging_in: "正在提交登录凭据",
  reading_account: "正在读取余额",
  saving_account: "正在保存账号",
  done: "操作完成",
};

// ----------------------------------------------------------------------------
// Modals: Confirm Retry Dialog for Unknown Checkin
// ----------------------------------------------------------------------------
interface ConfirmRetryDialogProps {
  accountUsername: string;
  onClose: () => void;
  onConfirm: () => void;
}

export function ConfirmRetryDialog({
  accountUsername,
  onClose,
  onConfirm,
}: ConfirmRetryDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelBtnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    cancelBtnRef.current?.focus();
  }, []);

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), [tabindex="0"]'
    );
    if (!focusable?.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <div className="modal-backdrop" onClick={e => e.target === e.currentTarget && onClose()}>
      <div
        className="card modal-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-retry-title"
        ref={dialogRef}
        onKeyDown={handleKeyDown}
      >
        <div className="modal-header">
          <div className="modal-title-group">
            <div className="modal-icon-badge modal-icon-warn">
              <AlertTriangle size={18} />
            </div>
            <div>
              <h2 id="confirm-retry-title" className="modal-title">确认重试签到</h2>
              <p className="modal-subtitle">账号「{accountUsername}」</p>
            </div>
          </div>
          <button type="button" className="btn-icon" onClick={onClose} aria-label="关闭对话框">
            <X size={18} />
          </button>
        </div>

        <div className="modal-body">
          <p className="modal-text secondary">
            该账号上次签到结果未知（如上游超时或未返回明确状态），账号「{accountUsername}」再次签到可能触发重复请求。请确认是否继续重试。
          </p>
        </div>

        <div className="modal-footer">
          <button type="button" className="btn btn-secondary" ref={cancelBtnRef} onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="btn btn-warn"
            onClick={() => {
              onConfirm();
              onClose();
            }}
          >
            确认继续重试
          </button>
        </div>
      </div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Modals: Reveal Key Dialog (Root Authenticated, 30s Auto-Clear)
// ----------------------------------------------------------------------------
interface RevealKeyDialogProps {
  accountId: string;
  accountUsername: string;
  onClose: () => void;
  onReveal: (accountId: string, rootKey: string, signal?: AbortSignal) => Promise<string> | string;
}

export function RevealKeyModal({ accountId, accountUsername, onClose, onReveal }: RevealKeyDialogProps) {
  const [rootKey, setRootKey] = useState("");
  const [revealedKey, setRevealedKey] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState("");
  const [copyFeedback, setCopyFeedback] = useState("");
  const [countdown, setCountdown] = useState(30);

  const generationRef = useRef(0);
  const isAliveRef = useRef(true);

  const dialogRef = useRef<HTMLDivElement>(null);
  const rootInputRef = useRef<HTMLInputElement>(null);
  const autoHideTimerRef = useRef<number | null>(null);
  const countdownIntervalRef = useRef<number | null>(null);
  const copyTimerRef = useRef<number | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  function clearTimers() {
    if (autoHideTimerRef.current !== null) window.clearTimeout(autoHideTimerRef.current);
    if (countdownIntervalRef.current !== null) window.clearInterval(countdownIntervalRef.current);
    if (copyTimerRef.current !== null) window.clearTimeout(copyTimerRef.current);
    autoHideTimerRef.current = countdownIntervalRef.current = copyTimerRef.current = null;
  }

  useEffect(() => {
    isAliveRef.current = true;
    setRootKey("");
    setRevealedKey(null);
    setErrorMsg("");
    setCopyFeedback("");
    setSubmitting(false);
    setCountdown(30);
    rootInputRef.current?.focus();
    return () => {
      isAliveRef.current = false;
      generationRef.current += 1;
      abortRef.current?.abort();
      clearTimers();
    };
  }, [accountId]);

  function handleClose() {
    generationRef.current += 1;
    setRootKey("");
    setRevealedKey(null);
    setErrorMsg("");
    setCopyFeedback("");
    setSubmitting(false);
    abortRef.current?.abort();
    clearTimers();
    onClose();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      handleClose();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), a[href], [tabindex="0"]'
    );
    if (!focusable?.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (!rootKey.trim() || submitting) return;

    const epoch = ++generationRef.current;
    setSubmitting(true);
    setErrorMsg("");

    const task = new AbortController();
    abortRef.current = task;
    let secret = rootKey;
    setRootKey("");

    try {
      const pending = onReveal(accountId, secret, task.signal);
      secret = "";
      const key = await pending;
      if (!isAliveRef.current || epoch !== generationRef.current || task.signal.aborted) {
        return;
      }
      setRevealedKey(key);
      setCountdown(30);

      if (countdownIntervalRef.current) window.clearInterval(countdownIntervalRef.current);
      countdownIntervalRef.current = window.setInterval(() => {
        if (!isAliveRef.current || epoch !== generationRef.current) {
          if (countdownIntervalRef.current) window.clearInterval(countdownIntervalRef.current);
          return;
        }
        setCountdown(prev => {
          if (prev <= 1) {
            if (countdownIntervalRef.current) window.clearInterval(countdownIntervalRef.current);
            return 0;
          }
          return prev - 1;
        });
      }, 1000);

      if (autoHideTimerRef.current) window.clearTimeout(autoHideTimerRef.current);
      autoHideTimerRef.current = window.setTimeout(() => {
        if (!isAliveRef.current || epoch !== generationRef.current) return;
        generationRef.current += 1;
        clearTimers();
        setCopyFeedback("");
        setRevealedKey(null);
        setErrorMsg("已自动隐藏密钥。如需查看，请重新验证");
      }, 30000);
    } catch (err: unknown) {
      if (!isAliveRef.current || epoch !== generationRef.current || task.signal.aborted) {
        return;
      }
      setErrorMsg(err instanceof Error ? err.message : "验证失败，请确认密钥");
    } finally {
      secret = "";
      if (isAliveRef.current && epoch === generationRef.current) {
        setSubmitting(false);
      }
    }
  }

  async function handleCopy() {
    if (!revealedKey) return;
    const epoch = generationRef.current;
    try {
      await navigator.clipboard.writeText(revealedKey);
      if (!isAliveRef.current || epoch !== generationRef.current) return;
      setCopyFeedback("已复制到剪贴板");
      if (copyTimerRef.current !== null) window.clearTimeout(copyTimerRef.current);
      copyTimerRef.current = window.setTimeout(() => {
        if (isAliveRef.current && epoch === generationRef.current) setCopyFeedback("");
      }, 2000);
    } catch {
      if (!isAliveRef.current || epoch !== generationRef.current) return;
      setCopyFeedback("复制失败，请手动选取复制");
    }
  }

  return (
    <div className="modal-backdrop" onClick={e => e.target === e.currentTarget && handleClose()}>
      <div
        className="card modal-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="reveal-key-title"
        ref={dialogRef}
        onKeyDown={handleKeyDown}
      >
        <div className="modal-header">
          <div className="modal-title-group">
            <div className="modal-icon-badge">
              <KeyRound size={18} />
            </div>
            <div>
              <h2 id="reveal-key-title" className="modal-title">查看密钥</h2>
              <p className="modal-subtitle">账号：{accountUsername}</p>
            </div>
          </div>
          <button type="button" className="btn-icon" onClick={handleClose} aria-label="关闭对话框">
            <X size={18} />
          </button>
        </div>

        <div className="modal-body">
          {errorMsg && (
            <div className="alert alert-error" role="alert">
              <AlertCircle size={16} className="alert-icon" />
              <div className="alert-content">{errorMsg}</div>
            </div>
          )}

          {!revealedKey ? (
            <form className="stack" onSubmit={handleSubmit}>
              <div className="field">
                <label htmlFor="reveal-root-key" className="field-label">
                  密钥
                </label>
                <input
                  id="reveal-root-key"
                  ref={rootInputRef}
                  type="password"
                  className="text-input"
                  autoComplete="off"
                  required
                  placeholder="输入密钥"
                  value={rootKey}
                  onChange={e => setRootKey(e.target.value)}
                  disabled={submitting}
                />
              </div>

              <div className="modal-footer">
                <button
                  type="button"
                  className="btn btn-secondary"
                  onClick={handleClose}
                  disabled={submitting}
                >
                  关闭
                </button>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={submitting || !rootKey.trim()}
                >
                  {submitting ? (
                    <>
                      <Loader2 size={16} className="spin" />
                      <span>正在验证…</span>
                    </>
                  ) : (
                    <>
                      <ShieldCheck size={16} />
                      <span>验证并显示</span>
                    </>
                  )}
                </button>
              </div>
            </form>
          ) : (
            <div className="stack">
              <div className="field">
                <div className="field-header">
                  <label htmlFor="revealed-key-value" className="field-label">
                    当前密钥（{countdown}s 后自动隐藏）
                  </label>
                  <span className="text-teal text-xs font-mono tabular-nums">{countdown}s</span>
                </div>
                <div className="input-with-button">
                  <input
                    id="revealed-key-value"
                    type="text"
                    className="text-input font-mono text-sm"
                    readOnly
                    value={revealedKey}
                  />
                  <button type="button" className="btn btn-secondary" onClick={handleCopy}>
                    <Copy size={16} />
                    <span>复制</span>
                  </button>
                </div>
                {copyFeedback && (
                  <span className="feedback text-teal text-xs" role="status">
                    {copyFeedback}
                  </span>
                )}
              </div>

              <div className="modal-footer">
                <button type="button" className="btn btn-primary" onClick={handleClose}>
                  关闭
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ----------------------------------------------------------------------------
// Main Component: AccountsPanel
// ----------------------------------------------------------------------------
export interface ExtendedAccountsPanelProps extends AccountsPanelProps {
  candidate?: CandidateDTO | null;
  candidateExpired?: boolean;
  onSaveCandidate?: (candidateId: string, keyId: string | null) => Promise<void>;
  operation?: OperationDTO | null;
  onNavigateToSettings?: () => void;
  routeError?: string | null;
  accountCheckin?: AccountCheckinResponse | null;
}

export function AccountsPanel({
  accounts,
  routedAccountId,
  pendingRouteAccountId,
  globalCheckinConfig = { enabled: false, startTime: "09:00", intervalMinutes: 30 },
  busy = false,
  onSelectRoute,
  onManualCheckin,
  onAddAccount,
  onSelectKey,
  onRefreshBalance,
  onRevealKey,
  onUpdateGlobalCheckin,
  onViewDetails,
  selectedDetailAccountId,
  hideGlobalCheckin = false,
  error,
  candidate = null,
  candidateExpired = false,
  onSaveCandidate,
  operation = null,
  onNavigateToSettings,
  routeError,
  requestLogs = [],
  refreshingAccountId = null,
  accountRefreshError = {},
  accountCheckin = null,
  quotaAutoRefreshEnabled = true,
  quotaRefreshIntervalMinutes = 10,
  onToggleQuotaAutoRefresh,
  onChangeQuotaRefreshInterval,
}: ExtendedAccountsPanelProps) {
  const [showAddModal, setShowAddModal] = useState(() => Boolean(candidate));
  const [confirmRetryAccount, setConfirmRetryAccount] = useState<ManagedAccount | null>(null);
  const [revealKeyAccount, setRevealKeyAccount] = useState<ManagedAccount | null>(null);
  const [localExpandedId, setLocalExpandedId] = useState<string | null>(null);

  const prevCandidateRef = useRef<CandidateDTO | null>(candidate);
  useEffect(() => {
    if (candidate && !prevCandidateRef.current) {
      setShowAddModal(true);
    }
    prevCandidateRef.current = candidate;
  }, [candidate]);

  const activeDetailId = selectedDetailAccountId !== undefined ? selectedDetailAccountId : localExpandedId;
  const handleToggleDetail = (accountId: string) => {
    if (onViewDetails) {
      onViewDetails(accountId);
    }
    setLocalExpandedId(prev => (prev === accountId ? null : accountId));
  };

  // If candidate arrives while modal is closed, allow resuming via header button
  const hasActiveCandidate = !!candidate && !candidateExpired;
  const isLoggingIn = operation?.kind === "login" && operation?.status === "running";

  return (
    <section className="accounts-panel-section stack" aria-labelledby="accounts-panel-title">
      {/* Page Header: Title on Left, Add Account on Right */}
      <div className="accounts-list-header">
        <div className="accounts-title-group">
          <h2 id="accounts-panel-title" className="section-title">已绑定账号列表</h2>
          <span className="text-secondary text-sm font-mono tabular-nums count-badge">
            共 {accounts.length} 个账号
          </span>
        </div>
        <div className="accounts-header-actions">
          {/* Polished Compact Quota Auto-Refresh Controls */}
          <div className="quota-auto-refresh-controls flex items-center gap-2">
            <label className="auto-refresh-toggle flex items-center gap-1.5 cursor-pointer text-xs font-medium text-secondary">
              <input
                type="checkbox"
                aria-label="自动刷新余额"
                checked={quotaAutoRefreshEnabled}
                onChange={e => onToggleQuotaAutoRefresh?.(e.target.checked)}
                className="auto-refresh-checkbox"
              />
              <span>自动刷新</span>
            </label>
            <select
              aria-label="自动刷新间隔"
              value={quotaRefreshIntervalMinutes}
              disabled={!quotaAutoRefreshEnabled}
              onChange={e => onChangeQuotaRefreshInterval?.(Number(e.target.value) as 5 | 10)}
              className="select-sm font-mono text-xs"
            >
              <option value={5}>每 5 分钟</option>
              <option value={10}>每 10 分钟</option>
            </select>
            <span
              className="auto-refresh-hint text-xs text-secondary"
              title="仅在浏览器当前标签页处于前台激活时自动执行刷新"
            >
              （仅页面开启时）
            </span>
          </div>

          {hasActiveCandidate && (
            <button
              type="button"
              className="btn btn-accent btn-sm"
              onClick={() => setShowAddModal(true)}
            >
              <UserCheck size={15} />
              <span>候选账号就绪</span>
            </button>
          )}

          {isLoggingIn && (
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => setShowAddModal(true)}
            >
              <Loader2 size={15} className="spin text-teal" />
              <span>登录中…</span>
            </button>
          )}

          <button
            type="button"
            className="btn btn-primary btn-sm"
            disabled={busy && !isLoggingIn}
            onClick={() => setShowAddModal(true)}
          >
            <Plus size={15} />
            <span>添加账号</span>
          </button>
        </div>
      </div>

      {error && !error.includes("路由") && (
        <div className="alert alert-error" role="alert" style={{ marginBottom: "1rem" }}>
          <AlertCircle size={16} className="alert-icon" />
          <div className="alert-content">{error}</div>
        </div>
      )}

      {/* Legacy compatibility for unit tests expecting global checkin form inside AccountsPanel */}
      {!hideGlobalCheckin && onUpdateGlobalCheckin && (
        <GlobalCheckinCard
          globalConfig={globalCheckinConfig}
          busy={busy}
          onSave={onUpdateGlobalCheckin}
        />
      )}


      {accounts.length === 0 ? (
        <div className="empty-state-card card">
          <div className="empty-icon-wrapper">
            <Users size={32} className="text-secondary" />
          </div>
          <h4 className="empty-title">尚未添加任何账号</h4>
          <p className="empty-description">
            请点击右上角「添加账号」开始配置。当前尚未指定路由账号。
          </p>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => setShowAddModal(true)}
          >
            <Plus size={16} />
            <span>立即添加账号</span>
          </button>
        </div>
      ) : (
        <div className="accounts-table-wrapper table-responsive">
          <table className="accounts-table data-table" aria-label="账号列表">
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
                const checkinStatus = account.checkin?.status || "pending";
                const isCheckinDone =
                  checkinStatus === "success" || checkinStatus === "already_done";
                const isCheckinRunning = checkinStatus === "running";
                const isCheckinFailed = checkinStatus === "failed";
                const isRefreshing = refreshingAccountId === account.id;
                const refreshError = accountRefreshError[account.id];

                // Local request stats from retained logs
                const accountLogs = (requestLogs || []).filter(
                  l => l.accountId === account.id || l.accountName === account.username
                );
                const totalCalls = accountLogs.length;
                const successCalls = accountLogs.filter(
                  l => l.httpStatus !== null && l.httpStatus >= 200 && l.httpStatus < 300
                ).length;

                // Contextual operation error for this specific account
                const accountOpError =
                  operation?.status === "failed" && operation?.account_id === account.id
                    ? (operation.error ? errorText(operation.error) : "操作失败")
                    : null;

                const isAccountOpRunning =
                  operation?.account_id === account.id && operation?.status === "running";

                const isSelectedDetail = activeDetailId === account.id;

                return (
                  <Fragment key={account.id}>
                    <tr
                      data-testid={`account-card-${account.id}`}
                      className={`account-row account-card cursor-pointer ${isRouted ? "is-routed-row is-routed-card" : ""} ${isSelectedDetail ? "is-selected-row" : ""}`}
                      onClick={() => handleToggleDetail(account.id)}
                    >
                      {/* 1. 名称 */}
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
                            {operation?.status === "running" &&
                              (operation.account_id === account.id ||
                                (!operation.account_id && operation.kind === "refresh")) && (
                              <span className="badge badge-warn text-xs inline-flex items-center gap-1">
                                <Loader2 size={10} className="spin" />
                                <span>{operation.phase ? (phaseLabelsLocal[operation.phase] || "正在处理…") : "正在处理…"}</span>
                              </span>
                            )}
                            {account.checkin?.status === "success" && (
                              <span className="badge badge-success text-xs font-medium">已签到</span>
                            )}
                            {account.checkin?.status === "already_done" && (
                              <span className="badge badge-success text-xs font-medium">今日已完成</span>
                            )}
                            {account.checkin?.status === "unknown" && (
                              <span className="badge badge-warn text-xs font-medium">待确认签到</span>
                            )}
                            {account.checkin?.status === "failed" && (
                              <span className="badge badge-error text-xs font-medium">签到失败</span>
                            )}
                          </div>
                          {refreshError && (
                            <div className="text-error text-xs inline-flex items-center gap-1" style={{ marginTop: "2px" }}>
                              <AlertCircle size={12} />
                              <span>{refreshError}</span>
                            </div>
                          )}
                        </div>
                      </td>

                      {/* 2. 调用情况 */}
                      <td className="col-activity tabular-nums">
                        <div className="activity-cell-content">
                          {totalCalls === 0 ? (
                            <span className="text-secondary text-xs">—</span>
                          ) : (
                            <span className="text-xs inline-flex items-center gap-1">
                              <span className={`status-dot ${totalCalls === successCalls ? "dot-success" : "dot-warn"}`} />
                              <span>{successCalls}/{totalCalls} 成功</span>
                            </span>
                          )}
                        </div>
                      </td>

                      {/* 3. 剩余额度 (USD) */}
                      <td className="col-quota tabular-nums">
                        <div className="font-semibold text-teal font-mono">
                          {formatRemainingUsd(account.balance.quotaRaw, account.balance.usedQuotaRaw)}
                        </div>
                      </td>

                      {/* 4. 已用额度 (USD) */}
                      <td className="col-quota tabular-nums">
                        <div className="text-secondary font-mono">
                          {formatQuotaUsd(account.balance.usedQuotaRaw)}
                        </div>
                      </td>

                      {/* 5. 更新时间 */}
                      <td className="col-time tabular-nums text-xs text-secondary font-mono">
                        {formatShanghaiDateTime(account.balance.fetchedAt)}
                      </td>

                      {/* 6. 操作 (Row actions do not bubble!) */}
                      <td className="col-actions text-right" onClick={e => e.stopPropagation()}>
                        <div className="account-actions-cell">
                          {onRefreshBalance && (
                            <button
                              type="button"
                              className="icon-action-btn"
                              disabled={busy || isRefreshing}
                              onClick={() => onRefreshBalance(account.id)}
                              title="刷新该账号额度"
                              aria-label="刷新余额"
                            >
                              <RefreshCw size={14} className={isRefreshing || isAccountOpRunning ? "spin text-teal" : ""} />
                            </button>
                          )}

                          {onViewDetails && (
                            <button
                              type="button"
                              className="icon-action-btn detail-toggle-btn"
                              aria-label={isSelectedDetail ? "收起详情" : "查看详情"}
                              title={isSelectedDetail ? "收起详情" : "查看详情"}
                              onClick={() => handleToggleDetail(account.id)}
                            >
                              <span className="text-xs">{isSelectedDetail ? "收起" : "详情"}</span>
                              <ChevronRight size={14} className={`chevron-icon ${isSelectedDetail ? "rotated-down" : ""}`} />
                            </button>
                          )}
                        </div>
                      </td>
                    </tr>

                    {/* Expandable Detail Drawer Row */}
                    {isSelectedDetail && (
                      <tr
                        data-testid={`account-detail-drawer-${account.id}`}
                        className="account-detail-drawer-row"
                      >
                        <td colSpan={6} className="detail-drawer-cell">
                          <div className="account-detail-drawer stack compact">
                            {/* Drawer Header */}
                            <div className="detail-drawer-header flex items-center justify-between">
                              <div className="flex items-center gap-2">
                                <h3 id="account-detail-title" className="detail-drawer-title font-semibold text-primary">
                                  账号详情 · {account.username}
                                </h3>
                                <span className="text-secondary text-xs font-mono">
                                  上游用户 ID：{account.upstreamUserId}
                                </span>
                              </div>
                              <button
                                type="button"
                                className="btn btn-secondary btn-xs"
                                onClick={() => handleToggleDetail(account.id)}
                              >
                                收起详情
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
                                  刷新时间：{account.balance?.fetchedAt ? formatShanghaiDateTime(account.balance.fetchedAt) : "尚未刷新"}
                                </span>
                              </div>
                              <div className="quota-detail-grid">
                                <div className="card quota-detail-item">
                                  <span className="text-secondary text-xs">剩余额度</span>
                                  <div className="flex items-baseline gap-2">
                                    <span className="text-base font-bold font-mono text-teal tabular-nums">
                                      {formatRemainingUsd(account.balance?.quotaRaw, account.balance?.usedQuotaRaw)}
                                    </span>
                                    <span className="text-xs text-secondary font-mono">
                                      (原始 raw: {account.balance?.quotaRaw || "—"})
                                    </span>
                                  </div>
                                </div>
                                <div className="card quota-detail-item">
                                  <span className="text-secondary text-xs">已用额度</span>
                                  <div className="flex items-baseline gap-2">
                                    <span className="text-base font-bold font-mono text-secondary tabular-nums">
                                      {formatQuotaUsd(account.balance?.usedQuotaRaw)}
                                    </span>
                                    <span className="text-xs text-secondary font-mono">
                                      (原始 raw: {account.balance?.usedQuotaRaw || "—"})
                                    </span>
                                  </div>
                                </div>
                              </div>
                              <div className="text-xs text-secondary flex items-center justify-between">
                                <span>换算说明：按当前 500,000 raw / USD 参考计算，具体核算口径待上游确认。</span>
                              </div>
                            </div>

                            {/* Action Controls Bar */}
                            <div className="detail-actions-bar card subcard flex items-center justify-between flex-wrap gap-3">
                              {/* Key Management */}
                              <div className="flex items-center gap-2 flex-wrap">
                                <div className="flex items-center gap-1.5 text-xs text-secondary">
                                  <Key size={14} />
                                  <span>当前密钥：</span>
                                  <span className="font-mono text-primary font-medium">
                                    {account.selectedKey ? `${account.selectedKey.name} (${account.selectedKey.masked})` : "未关联密钥"}
                                  </span>
                                </div>

                                {onSelectKey && account.keys.length > 1 && (
                                  <div className="flex items-center gap-1">
                                    <label htmlFor={`key-select-${account.id}`} className="text-xs text-secondary sr-only">
                                      切换密钥
                                    </label>
                                    <select
                                      id={`key-select-${account.id}`}
                                      className="select-input select-xs"
                                      aria-label="切换密钥"
                                      value={account.selectedKey?.id || ""}
                                      disabled={busy}
                                      onChange={e => onSelectKey(account.id, e.target.value)}
                                    >
                                      {account.keys.map(k => (
                                        <option key={k.id} value={k.id}>
                                          {k.name} ({k.masked})
                                        </option>
                                      ))}
                                    </select>
                                  </div>
                                )}

                                {onRevealKey && (
                                  <button
                                    type="button"
                                    className="btn btn-secondary btn-xs"
                                    disabled={busy || !hasKey}
                                    onClick={() => setRevealKeyAccount(account)}
                                  >
                                    <Eye size={13} />
                                    <span>查看密钥</span>
                                  </button>
                                )}
                              </div>

                              {/* Route & Checkin Triggers */}
                              <div className="flex items-center gap-2 flex-wrap">
                                {/* Manual Checkin */}
                                {(onManualCheckin || isCheckinDone) && (
                                  <button
                                    type="button"
                                    className="btn btn-secondary btn-xs"
                                    disabled={busy || isCheckinDone || isCheckinRunning || !onManualCheckin}
                                    aria-label={isCheckinDone ? "今日已完成" : undefined}
                                    onClick={() => {
                                      if (checkinStatus === "unknown") {
                                        setConfirmRetryAccount(account);
                                      } else if (onManualCheckin) {
                                        onManualCheckin(account.id, { confirmRetry: false });
                                      }
                                    }}
                                  >
                                    {isCheckinRunning ? (
                                      <>
                                        <Loader2 size={13} className="spin" />
                                        <span>正在签到</span>
                                      </>
                                    ) : isCheckinDone ? (
                                      <>
                                        <Check size={13} />
                                        <span>今日已完成</span>
                                      </>
                                    ) : checkinStatus === "unknown" || isCheckinFailed ? (
                                      <>
                                        <HelpCircle size={13} />
                                        <span>重试签到</span>
                                      </>
                                    ) : (
                                      <>
                                        <Calendar size={13} />
                                        <span>手动签到</span>
                                      </>
                                    )}
                                  </button>
                                )}

                                {/* Route Button */}
                                <div className="inline-block">
                                  <button
                                    type="button"
                                    className={`btn btn-xs ${isRouted ? "btn-route-active badge-success" : "btn-primary"}`}
                                    disabled={busy || isRouted || !hasKey || isPendingRoute}
                                    onClick={() => onSelectRoute && onSelectRoute(account.id)}
                                  >
                                    {isRouted ? (
                                      <>
                                        <Check size={13} />
                                        <span>当前正在使用</span>
                                      </>
                                    ) : isPendingRoute ? (
                                      <>
                                        <Loader2 size={13} className="spin" />
                                        <span>正在切换路由…</span>
                                      </>
                                    ) : !hasKey ? (
                                      <span>未选择密钥</span>
                                    ) : (
                                      <span>使用此账号</span>
                                    )}
                                  </button>
                                  {!hasKey && (
                                    <div className="text-secondary text-xs" style={{ marginTop: "2px" }}>
                                      此账号暂未关联有效密钥
                                    </div>
                                  )}
                                </div>
                              </div>
                            </div>

                            {/* Inline Error Feedback */}
                            {routeError && (
                              <div className="alert alert-error text-xs" role="alert">
                                <AlertCircle size={14} className="alert-icon" />
                                <span>{routeError}</span>
                              </div>
                            )}
                            {refreshError && (
                              <div className="alert alert-error text-xs" role="alert">
                                <AlertCircle size={14} className="alert-icon" />
                                <span>余额刷新失败：{refreshError}</span>
                              </div>
                            )}
                            {accountOpError && (
                              <div className="alert alert-error text-xs" role="alert">
                                <AlertCircle size={14} className="alert-icon" />
                                <span>{accountOpError}</span>
                              </div>
                            )}

                            {/* Account-Specific Request Logs */}
                            <div className="account-subcard stack compact">
                              <div className="subcard-header flex items-center justify-between">
                                <span className="subcard-label flex items-center gap-1.5 text-xs font-semibold text-secondary">
                                  <FileText size={14} />
                                  <span>该账号本地网关调用记录</span>
                                </span>
                                <span className="text-secondary text-xs font-mono">
                                  {totalCalls} 次调用 · {successCalls} 次成功
                                </span>
                              </div>

                              {accountLogs.length === 0 ? (
                                <p className="secondary small text-center p-3">
                                  暂无此账号的网关调用记录
                                </p>
                              ) : (
                                <div className="log-table-wrapper table-responsive" style={{ maxHeight: "160px", overflowY: "auto" }}>
                                  <table className="log-table data-table text-xs" aria-label="账号专属请求记录">
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
                              )}
                            </div>

                            {/* Account Checkin History */}
                            {accountCheckin && accountCheckin.account_id === account.id && (
                              <div className="account-subcard stack compact">
                                <div className="subcard-header flex items-center justify-between">
                                  <span className="subcard-label flex items-center gap-1.5 text-xs font-semibold text-secondary">
                                    <Calendar size={14} />
                                    <span>签到历史记录（当前周期：{accountCheckin.cycle_date}）</span>
                                  </span>
                                </div>

                                <div className="checkin-table-wrapper table-responsive" style={{ maxHeight: "180px", overflowY: "auto" }}>
                                  <table className="checkin-table data-table text-xs" aria-label="账号签到历史">
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
                                      {(accountCheckin.history || []).length === 0 ? (
                                        <tr>
                                          <td colSpan={6} className="text-center secondary small">
                                            暂无历史签到记录
                                          </td>
                                        </tr>
                                      ) : (
                                        (accountCheckin.history || []).map((item: AccountCheckinRecord, idx: number) => (
                                          <tr key={`${item.date}-${idx}`}>
                                            <td className="font-mono">{formatCstDate(item.date)}</td>
                                            <td>{item.trigger === "manual" ? "手动触发" : "自动调度"}</td>
                                            <td>
                                              <span className={`badge ${item.status === "success" || item.status === "already_done" ? "badge-success" : item.status === "failed" ? "badge-error" : "badge-warn"} text-xs`}>
                                                {item.status === "success" ? "成功" : item.status === "already_done" ? "已完成" : item.status === "failed" ? "失败" : "未知"}
                                              </span>
                                            </td>
                                            <td className="font-mono">{formatShanghaiDateTime(item.started_at)}</td>
                                            <td className="font-mono">{formatShanghaiDateTime(item.finished_at)}</td>
                                            <td className="font-mono">{item.code || "—"}</td>
                                          </tr>
                                        ))
                                      )}
                                    </tbody>
                                  </table>
                                </div>
                              </div>
                            )}
                          </div>
                        </td>
                      </tr>
                    )}
                    {accountOpError && (
                      <tr className="account-error-row">
                        <td colSpan={6}>
                          <div className="alert alert-error text-xs" role="alert">
                            <AlertCircle size={14} className="alert-icon" />
                            <span>{accountOpError}</span>
                          </div>
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {/* Add Account Modal */}
      <AddAccountModal
        isOpen={showAddModal}
        onClose={() => setShowAddModal(false)}
        onLogin={async creds => {
          if (onAddAccount) await onAddAccount(creds);
        }}
        candidate={candidate}
        onSaveCandidate={async (candidateId, keyId) => {
          if (onSaveCandidate) await onSaveCandidate(candidateId, keyId);
        }}
        operation={operation}
        busy={busy}
        candidateExpired={candidateExpired}
      />

      {/* Unknown Retry Confirm Modal */}
      {confirmRetryAccount && (
        <ConfirmRetryDialog
          accountUsername={confirmRetryAccount.username}
          onClose={() => setConfirmRetryAccount(null)}
          onConfirm={() => {
            if (onManualCheckin) {
              onManualCheckin(confirmRetryAccount.id, { confirmRetry: true });
            }
          }}
        />
      )}

      {/* Key Reveal Modal */}
      {revealKeyAccount && onRevealKey && (
        <RevealKeyModal
          accountId={revealKeyAccount.id}
          accountUsername={revealKeyAccount.username}
          onClose={() => setRevealKeyAccount(null)}
          onReveal={(id, root, sig) => onRevealKey(id, root, sig)}
        />
      )}
    </section>
  );
}
