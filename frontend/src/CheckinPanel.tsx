import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";

export interface CheckinSettings {
  enabled: boolean;
  time: string; // HH:MM
  timezone: "Asia/Shanghai";
  reset_time?: "08:00";
}

export const defaultCheckinSettings: CheckinSettings = {
  enabled: false,
  time: "09:00",
  timezone: "Asia/Shanghai",
  reset_time: "08:00",
};

export type CheckinStatus = "running" | "success" | "already_done" | "failed" | "unknown";
export type CheckinTrigger = "manual" | "scheduled";

export interface CheckinRecord {
  date: string; // YYYY-MM-DD cycle date from server (08:00 - next day 08:00)
  accountUserId?: string;
  account_user_id?: string;
  trigger: CheckinTrigger;
  status: CheckinStatus;
  startedAt?: string;
  started_at?: string;
  finishedAt?: string | null;
  finished_at?: string | null;
  errorCode?: string | null;
  code?: string | null;
  message?: string | null;
}

export interface CheckinPanelProps {
  settings?: CheckinSettings | null;
  today: CheckinRecord | null;
  history: CheckinRecord[];
  nextRunAt?: string | null;
  cycleDate?: string | null;
  accountConfigured?: boolean;
  busy?: boolean;
  saving?: boolean;
  onSave?: (settings: { enabled: boolean; time: string; timezone?: "Asia/Shanghai" }) => Promise<void> | void;
  onUpdateSettings?: (settings: CheckinSettings) => Promise<void> | void;
  onRun: (options: { confirmRetry: boolean }) => Promise<void> | void;
  fetchError?: string;
  runError?: string;
  onRetryRead?: () => void;
}

export function formatShanghaiDateTime(iso: string | null | undefined): string {
  if (!iso) return "-";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
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

export const statusLabels: Record<CheckinStatus, string> = {
  running: "进行中",
  success: "今日已签到",
  already_done: "今日已完成",
  failed: "签到失败",
  unknown: "状态待确认",
};

export const triggerLabels: Record<CheckinTrigger, string> = {
  manual: "手动",
  scheduled: "定时",
};

export function getCheckinErrorMessage(code?: string | null, customMessage?: string | null): string {
  if (customMessage) return customMessage;
  if (!code) return "操作未完成，请重试";
  switch (code) {
    case "upstream_challenge":
      return "上游仍要求人机验证，签到尚未完成（受单浏览器限制，自动化非必然通过）";
    case "upstream_session_expired":
    case "session_expired":
      return "登录状态已过期，请重新登录账号";
    case "upstream_unavailable":
    case "upstream_5xx":
      return "上游服务暂时不可用或网络超时";
    case "upstream_unexpected_response":
    case "schema_mismatch":
      return "上游返回未预期的格式";
    case "invalid_credentials":
      return "账号凭据无效，请重新登录";
    case "account_not_configured":
      return "尚未配置激活账号，无法执行签到";
    case "operation_in_progress":
      return "已有后台任务正在运行中";
    case "checkin_retry_confirmation_required":
      return "当前周期签到结果处于未确认状态，重试可能导致重复提交，请确认后重试";
    case "network_error":
      return "网络连接异常，请重试";
    case "already_checked_in":
      return "上游记录显示当前周期已完成签到";
    case "intent_persist_failed":
      return "本地写入签到意图失败，未向外部发起请求";
    default:
      return "操作未完成，请重试";
  }
}

function StatusBadge({ status }: { status: CheckinStatus }) {
  const isOk = status === "success" || status === "already_done";
  const isErr = status === "failed";
  const isWarn = status === "unknown";
  const badgeClass = isOk ? "badge success" : isErr ? "badge error" : isWarn ? "badge warn" : "badge";
  return (
    <span className={badgeClass}>
      <span className="status-indicator" aria-hidden="true" />
      {statusLabels[status] || status}
    </span>
  );
}

interface ConfirmDialogProps {
  onClose: () => void;
  onConfirm: () => void;
}

function ConfirmDialog({ onClose, onConfirm }: ConfirmDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelBtnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    cancelBtnRef.current?.focus();
    return () => {
      previousFocus?.focus();
    };
  }, []);

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
      "button:not(:disabled), [tabindex='0']"
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
    <div className="modal-backdrop">
      <div
        className="card modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-dialog-title"
        ref={dialogRef}
        onKeyDown={handleKeyDown}
      >
        <div className="section-heading">
          <h2 id="confirm-dialog-title">确认重试签到</h2>
          <button type="button" onClick={onClose}>
            关闭
          </button>
        </div>
        <p className="secondary">
          上一次签到状态未知（如网络中断或 upstream 响应异常），再次签到可能触发重复请求。请确认是否继续重试。
        </p>
        <div className="actions">
          <button type="button" ref={cancelBtnRef} onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className="primary"
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

export function CheckinPanel({
  settings = defaultCheckinSettings,
  today,
  history,
  nextRunAt = null,
  cycleDate = null,
  accountConfigured = true,
  busy = false,
  saving = false,
  onSave,
  onUpdateSettings,
  onRun,
  fetchError,
  runError,
  onRetryRead,
}: CheckinPanelProps) {
  const currentSettings = settings || defaultCheckinSettings;
  const [enabled, setEnabled] = useState(currentSettings.enabled);
  const [time, setTime] = useState(currentSettings.time || "09:00");
  const [prevSettings, setPrevSettings] = useState(settings);
  const [expanded, setExpanded] = useState(false);
  const [showConfirm, setShowConfirm] = useState(false);
  const [saveFeedback, setSaveFeedback] = useState("");
  const feedbackTimer = useRef<number | null>(null);

  const enableId = useId();
  const timeId = useId();

  // Synchronize when settings prop updates
  if (settings !== prevSettings) {
    setPrevSettings(settings);
    if (settings) {
      setEnabled(settings.enabled);
      setTime(settings.time || "09:00");
    }
  }

  useEffect(() => {
    return () => {
      if (feedbackTimer.current) window.clearTimeout(feedbackTimer.current);
    };
  }, []);

  const timeRegex = /^(?:[01]\d|2[0-3]):[0-5]\d$/;
  const isTimeValid = timeRegex.test(time);
  const isDirty = enabled !== currentSettings.enabled || time !== currentSettings.time;

  const isTodayDone = today?.status === "success" || today?.status === "already_done";
  const isTodayRunning = today?.status === "running";
  const isManualDisabled =
    !accountConfigured || busy || isTodayDone || isTodayRunning;

  async function handleSave(event: FormEvent) {
    event.preventDefault();
    if (!isTimeValid || !isDirty || saving || busy) return;
    try {
      if (onUpdateSettings) {
        await onUpdateSettings({ enabled, time, timezone: "Asia/Shanghai" });
      } else if (onSave) {
        await onSave({ enabled, time, timezone: "Asia/Shanghai" });
      }
      setSaveFeedback("设置已保存");
      if (feedbackTimer.current) window.clearTimeout(feedbackTimer.current);
      feedbackTimer.current = window.setTimeout(() => setSaveFeedback(""), 3000);
    } catch {
      setSaveFeedback("保存失败，请重试");
    }
  }

  function triggerRun(confirmRetry: boolean) {
    void onRun({ confirmRetry });
  }

  function handleManualRun() {
    if (isManualDisabled) return;
    if (today?.status === "unknown") {
      setShowConfirm(true);
    } else {
      triggerRun(false);
    }
  }

  const currentCycleDate = cycleDate || today?.date;
  const visibleHistory = expanded ? history : history.slice(0, 10);

  return (
    <section className="card checkin-panel" aria-labelledby="checkin-panel-title">
      <div className="section-heading">
        <div>
          <h2 id="checkin-panel-title">每日自动签到</h2>
          <p className="secondary small">每天北京时间 08:00 更新签到机会</p>
        </div>
      </div>

      {fetchError && (
        <div className="operation-notice error-notice" role="alert">
          <span>读取签到状态失败：{fetchError}</span>
          {onRetryRead && (
            <button type="button" onClick={onRetryRead}>
              重试读取
            </button>
          )}
        </div>
      )}

      {runError && (
        <div className="inline-warning" role="alert">
          {runError}
        </div>
      )}

      {/* Manual Trigger & Current Cycle Overview */}
      <div className="subcard stack">
        <div className="checkin-today-header">
          <div>
            <span className="secondary small">
              当前签到周期 · {currentCycleDate ? `${currentCycleDate} 轮次` : "待读取周期"}
            </span>
            <div className="checkin-today-status">
              {today ? (
                <>
                  <StatusBadge status={today.status} />
                  <span className="secondary small numeric">
                    {(today.finishedAt ?? today.finished_at)
                      ? `完成时间：${formatShanghaiDateTime(today.finishedAt ?? today.finished_at)}`
                      : `开始时间：${formatShanghaiDateTime(today.startedAt ?? today.started_at)}`}
                  </span>
                </>
              ) : (
                <span className="secondary">当前周期尚未签到</span>
              )}
            </div>
          </div>

          <div className="checkin-action-group">
            <button
              type="button"
              className="primary"
              disabled={isManualDisabled}
              onClick={handleManualRun}
            >
              {!accountConfigured
                ? "立即签到"
                : isTodayDone
                ? "今日已完成"
                : isTodayRunning
                ? "正在签到…"
                : today?.status === "unknown"
                ? "重试签到"
                : today?.status === "failed"
                ? "重新签到"
                : "立即签到"}
            </button>
          </div>
        </div>

        {today && (today.status === "failed" || today.status === "unknown") && (
          <p className="inline-warning">
            {getCheckinErrorMessage(today.errorCode || today.code, today.message)}
          </p>
        )}

        {!accountConfigured && (
          <p className="secondary small">需先配置并激活当前账号后，方可执行签到。</p>
        )}
        {isTodayDone && (
          <p className="secondary small">当前周期已完成签到，无需重复操作。</p>
        )}
      </div>

      {/* Schedule Settings Form */}
      <form className="subcard stack" onSubmit={handleSave}>
        <div className="section-heading compact">
          <h3>自动签到设置</h3>
          <span className="secondary small numeric">北京时间（Asia/Shanghai）</span>
        </div>

        <div className="checkin-settings-grid">
          <label className="checkbox-label" htmlFor={enableId}>
            <input
              id={enableId}
              type="checkbox"
              checked={enabled}
              disabled={busy || saving}
              onChange={(e) => setEnabled(e.target.checked)}
            />
            <span>启用每日自动签到</span>
          </label>

          <div className="field checkin-time-field">
            <label htmlFor={timeId}>每日签到时间</label>
            <input
              id={timeId}
              type="time"
              value={time}
              disabled={busy || saving}
              onChange={(e) => setTime(e.target.value)}
              required
            />
          </div>
        </div>

        {!isTimeValid && (
          <span className="error-text small">
            请输入有效的 24 小时制时间（格式如 08:30）
          </span>
        )}

        <div className="checkin-plan-info">
          {enabled ? (
            <p className="secondary small numeric">
              {nextRunAt
                ? `下次计划执行时间：${formatShanghaiDateTime(nextRunAt)}（北京时间${time < "08:00" ? "，08:00 前执行属于上一签到轮" : ""}）`
                : `每日 ${time} 自动执行（北京时间，08:00 前执行属于上一签到轮）`}
            </p>
          ) : (
            <p className="secondary small">自动签到已停用</p>
          )}
        </div>

        <div className="actions">
          <button
            type="submit"
            disabled={!isDirty || !isTimeValid || busy || saving}
          >
            {saving ? "正在保存…" : "保存配置"}
          </button>
          {saveFeedback && (
            <span className="feedback" role="status">
              {saveFeedback}
            </span>
          )}
        </div>
      </form>

      {/* History Records Table */}
      <div className="stack compact">
        <div className="section-heading">
          <h3>签到记录</h3>
          <span className="secondary small numeric">共 {history.length} 条</span>
        </div>

        {history.length === 0 ? (
          <div className="empty-state">
            <p className="secondary">暂无签到历史记录</p>
          </div>
        ) : (
          <div className="checkin-table-wrapper">
            <table className="checkin-table">
              <thead>
                <tr>
                  <th scope="col">日期</th>
                  <th scope="col">触发方式</th>
                  <th scope="col">状态</th>
                  <th scope="col">完成时间</th>
                  <th scope="col">说明</th>
                </tr>
              </thead>
              <tbody>
                {visibleHistory.map((item, idx) => (
                  <tr key={`${item.date}-${item.startedAt}-${idx}`}>
                    <td className="numeric">{item.date}</td>
                    <td>{triggerLabels[item.trigger] || item.trigger}</td>
                    <td>
                      <StatusBadge status={item.status} />
                    </td>
                    <td className="numeric">
                      {formatShanghaiDateTime(item.finishedAt ?? item.finished_at)}
                    </td>
                    <td className="secondary small">
                      {item.status === "failed" || item.status === "unknown"
                        ? getCheckinErrorMessage(item.errorCode || item.code, item.message)
                        : item.message || "-"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        {history.length > 10 && (
          <div className="actions">
            <button
              type="button"
              className="text-btn"
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? "收起较早记录" : `查看较早记录（共 ${history.length} 条）`}
            </button>
          </div>
        )}
      </div>

      {showConfirm && (
        <ConfirmDialog
          onClose={() => setShowConfirm(false)}
          onConfirm={() => triggerRun(true)}
        />
      )}
    </section>
  );
}
