import { useState } from "react";
import { AlertCircle, ChevronDown, ChevronRight, RefreshCw, Server, Trash2 } from "lucide-react";
import type { RequestLogEntry } from "./accountsTypes";
import type { LogLevel, SystemEvent, SystemLogsResponse } from "./api";

export interface RequestLogPanelProps {
  logs: RequestLogEntry[];
  loading?: boolean;
  onRefresh?: () => Promise<void> | void;
  error?: string | null;
  systemLogs?: SystemLogsResponse | null;
  systemLogsLoading?: boolean;
  systemLogsError?: string | null;
  onRefreshSystemLogs?: (params?: { limit?: number; level?: string; account_id?: string; operation_id?: string }) => Promise<void> | void;
  onClearSystemLogs?: () => Promise<void> | void;
  onClearRequestLogs?: () => Promise<void> | void;
}

export function formatLogTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return new Intl.DateTimeFormat("zh-CN", {
    timeZone: "Asia/Shanghai",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(d);
}

function StatusPill({ status }: { status: number | null }) {
  if (status === null) {
    return <span className="badge badge-warn numeric">未收到响应</span>;
  }
  if (status >= 200 && status < 300) {
    return (
      <span className="badge badge-success numeric">
        <span className="status-indicator online" aria-hidden="true" />
        {status}
      </span>
    );
  }
  if (status >= 400 && status < 500) {
    return <span className="badge badge-warn numeric">{status}</span>;
  }
  return <span className="badge badge-error numeric">{status}</span>;
}

function LevelBadge({ level }: { level: LogLevel }) {
  const lvl = level.toLowerCase();
  if (lvl === "error") return <span className="badge badge-error font-mono text-xs">ERROR</span>;
  if (lvl === "warn") return <span className="badge badge-warn font-mono text-xs">WARN</span>;
  if (lvl === "info") return <span className="badge badge-success font-mono text-xs">INFO</span>;
  if (lvl === "debug") return <span className="badge font-mono text-xs" style={{ background: "rgba(148, 163, 184, 0.2)", color: "#94a3b8" }}>DEBUG</span>;
  return <span className="badge font-mono text-xs" style={{ background: "rgba(100, 116, 139, 0.2)", color: "#64748b" }}>TRACE</span>;
}

export function ForwardingModeBadge({ mode }: { mode?: "pass" | "adapt" | null }) {
  if (mode === "pass") {
    return (
      <span className="badge badge-neutral" title="透传模式（未修改请求体）">
        透传
      </span>
    );
  }
  if (mode === "adapt") {
    return (
      <span className="badge badge-accent" title="适配模式（已补全缺失的 prompt_cache_key）">
        反代
      </span>
    );
  }
  return (
    <span className="secondary small font-mono" title="未记录转发模式">
      —
    </span>
  );
}

export function RequestLogPanel({
  logs,
  loading = false,
  onRefresh,
  error = null,
  systemLogs,
  systemLogsLoading = false,
  systemLogsError = null,
  onRefreshSystemLogs,
  onClearSystemLogs,
  onClearRequestLogs,
}: RequestLogPanelProps) {
  const [activeTab, setActiveTab] = useState<"gateway" | "system">("gateway");

  // Gateway filters
  const [filter, setFilter] = useState<"all" | "errors" | "success">("all");
  const [expandedLogId, setExpandedLogId] = useState<string | null>(null);

  // System filters
  const [systemLevelFilter, setSystemLevelFilter] = useState<string>("all");
  const [expandedSystemLogId, setExpandedSystemLogId] = useState<string | null>(null);

  // Filtered gateway logs
  const filteredLogs = logs.filter(log => {
    if (filter === "errors") {
      return log.httpStatus === null || log.httpStatus >= 400;
    }
    if (filter === "success") {
      return log.httpStatus !== null && log.httpStatus >= 200 && log.httpStatus < 300;
    }
    return true;
  });

  // Filtered system logs
  const systemItems = systemLogs?.items || [];
  const filteredSystemLogs = systemItems.filter(item => {
    if (systemLevelFilter === "all") return true;
    return item.level.toLowerCase() === systemLevelFilter.toLowerCase();
  });

  return (
    <section className="card request-log-panel stack" aria-labelledby="request-logs-title">
      {/* Tab Switcher Header */}
      <div className="section-heading">
        <div className="tab-nav-bar flex gap-2">
          <button
            type="button"
            className={`tab-btn ${activeTab === "gateway" ? "active" : ""}`}
            onClick={() => setActiveTab("gateway")}
          >
            <span>网关请求日志</span>
            <span className="tab-count-badge numeric">({logs.length})</span>
          </button>
          <button
            type="button"
            className={`tab-btn ${activeTab === "system" ? "active" : ""}`}
            onClick={() => {
              setActiveTab("system");
              if (onRefreshSystemLogs && !systemLogs) {
                onRefreshSystemLogs();
              }
            }}
          >
            <span>系统运行日志</span>
            <span className="tab-count-badge numeric">({systemItems.length})</span>
          </button>
        </div>

        <div className="flex items-center gap-2">
          {activeTab === "gateway" && onRefresh && (
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={onRefresh}
              disabled={loading}
              aria-label={loading ? "正在刷新…" : "刷新日志"}
            >
              <RefreshCw size={14} className={loading ? "spin" : ""} />
              <span>{loading ? "正在刷新…" : "刷新日志"}</span>
            </button>
          )}

          {activeTab === "system" && onRefreshSystemLogs && (
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => onRefreshSystemLogs()}
              disabled={systemLogsLoading}
              aria-label="刷新系统日志"
            >
              <RefreshCw size={14} className={systemLogsLoading ? "spin" : ""} />
              <span>{systemLogsLoading ? "读取中…" : "刷新"}</span>
            </button>
          )}
        </div>
      </div>

      {/* --- Tab 1: Gateway Request Logs --- */}
      {activeTab === "gateway" && (
        <>
          <div className="section-heading compact">
            <div>
              <h2 id="request-logs-title" className="text-base font-semibold">网关请求日志</h2>
            </div>
            <div className="flex items-center gap-3">
              <div className="log-filters flex gap-1">
                <button
                  type="button"
                  className={`filter-btn ${filter === "all" ? "active" : ""}`}
                  onClick={() => setFilter("all")}
                >
                  全部
                </button>
                <button
                  type="button"
                  className={`filter-btn ${filter === "errors" ? "active" : ""}`}
                  onClick={() => setFilter("errors")}
                >
                  异常 (4xx/5xx)
                </button>
                <button
                  type="button"
                  className={`filter-btn ${filter === "success" ? "active" : ""}`}
                  onClick={() => setFilter("success")}
                >
                  成功 (2xx)
                </button>
              </div>
              <span className="secondary small tabular-nums font-mono">
                {filter === "all"
                  ? `共 ${logs.length} 条记录`
                  : `显示 ${filteredLogs.length} 条（共 ${logs.length} 条）`}
              </span>
            </div>
          </div>

          {error && (
            <div className="alert alert-error" role="alert">
              <span>{error}</span>
            </div>
          )}

          {logs.length === 0 ? (
            <div className="empty-state text-center p-6">
              <p className="secondary">暂无网关请求日志记录。</p>
            </div>
          ) : filteredLogs.length === 0 ? (
            <div className="empty-state text-center p-6">
              <p className="secondary">当前筛选条件下暂无请求记录</p>
            </div>
          ) : (
            <div className="log-table-wrapper table-responsive">
              <table className="log-table data-table" aria-label="网关请求日志列表">
                <thead>
                  <tr>
                    <th scope="col" style={{ width: "180px" }}>时间</th>
                    <th scope="col">所用账号</th>
                    <th scope="col" style={{ width: "90px" }}>转发模式</th>
                    <th scope="col" style={{ width: "110px" }}>状态码</th>
                    <th scope="col" style={{ width: "140px" }}>错误详情</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredLogs.map(log => {
                    const is2xx = log.httpStatus !== null && log.httpStatus >= 200 && log.httpStatus < 300;
                    const hasErrorBody = !is2xx && !!log.errorBody;
                    return (
                      <tr key={log.id} className="log-row">
                        <td className="numeric small font-mono">{formatLogTime(log.timestamp)}</td>
                        <td>
                          <strong>{log.accountName || "未指定"}</strong>
                        </td>
                        <td>
                          <ForwardingModeBadge mode={log.forwardingMode} />
                        </td>
                        <td>
                          <StatusPill status={log.httpStatus} />
                        </td>
                        <td>
                          {hasErrorBody ? (
                            <button
                              type="button"
                              className="btn btn-secondary btn-sm"
                              onClick={() =>
                                setExpandedLogId(expandedLogId === log.id ? null : log.id)
                              }
                              aria-expanded={expandedLogId === log.id}
                            >
                              {expandedLogId === log.id ? "收起原文" : "查看错误原文"}
                            </button>
                          ) : (
                            <span className="secondary small">—</span>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}

          {/* Expandable raw error body viewer: React text nodes only, never dangerouslySetInnerHTML */}
          {expandedLogId && (() => {
            const currentLog = logs.find(l => l.id === expandedLogId);
            if (!currentLog) return null;
            const hasBody = !!currentLog.errorBody;
            return (
              <div
                className="subcard stack compact log-detail-card"
                role="region"
                aria-label={`错误原文 · ${currentLog.accountName}`}
              >
                <div className="section-heading compact">
                  <span className="secondary small">
                    上游错误原文 · {formatLogTime(currentLog.timestamp)} · {currentLog.accountName}
                  </span>
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    onClick={() => setExpandedLogId(null)}
                  >
                    关闭
                  </button>
                </div>
                {currentLog.truncated && (
                  <span className="secondary small text-warn">
                    （提示：内容过长，已截断，仅保留前 64 KiB 原文）
                  </span>
                )}
                <pre className="log-error-body">
                  {hasBody ? currentLog.errorBody : "无错误正文"}
                </pre>
              </div>
            );
          })()}
        </>
      )}

      {/* --- Tab 2: System Logs (Fully wired to /api/system-logs) --- */}
      {activeTab === "system" && (
        <>
          <div className="section-heading compact">
            <div>
              <h2 className="text-base font-semibold">系统运行日志</h2>
            </div>
            <div className="log-filters flex gap-1">
              {["all", "error", "warn", "info", "debug"].map(lvl => (
                <button
                  key={lvl}
                  type="button"
                  className={`filter-btn ${systemLevelFilter === lvl ? "active" : ""}`}
                  onClick={() => setSystemLevelFilter(lvl)}
                >
                  {lvl.toUpperCase()}
                </button>
              ))}
            </div>
          </div>

          {systemLogsError && (
            <div className="alert alert-error" role="alert">
              <span>{systemLogsError}</span>
            </div>
          )}

          {systemItems.length === 0 ? (
            <div className="empty-state text-center p-6">
              <p className="secondary">当前暂无系统运行日志记录</p>
            </div>
          ) : filteredSystemLogs.length === 0 ? (
            <div className="empty-state text-center p-6">
              <p className="secondary">当前等级筛选下暂无记录</p>
            </div>
          ) : (
            <div className="log-table-wrapper table-responsive">
              <table className="log-table data-table" aria-label="系统运行日志列表">
                <thead>
                  <tr>
                    <th scope="col" style={{ width: "170px" }}>时间</th>
                    <th scope="col" style={{ width: "80px" }}>级别</th>
                    <th scope="col">事件</th>
                    <th scope="col" style={{ width: "120px" }}>阶段</th>
                    <th scope="col" style={{ width: "100px" }}>耗时</th>
                    <th scope="col">状态 / 原因</th>
                    <th scope="col" style={{ width: "100px" }}>详情</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredSystemLogs.map(item => {
                    const hasDiagnostics = !!item.diagnostics;
                    return (
                      <tr key={item.id} className="log-row">
                        <td className="numeric small font-mono">{formatLogTime(item.timestamp)}</td>
                        <td>
                          <LevelBadge level={item.level} />
                        </td>
                        <td>
                          <strong>{item.event}</strong>
                          {item.account_id && (
                            <span className="text-secondary text-xs block font-mono">
                              账号: {item.account_id}
                            </span>
                          )}
                        </td>
                        <td className="text-xs text-secondary font-mono">{item.stage || "—"}</td>
                        <td className="numeric text-xs font-mono">
                          {item.elapsed_ms !== null ? `${item.elapsed_ms}ms` : "—"}
                        </td>
                        <td>
                          {item.http_status && <StatusPill status={item.http_status} />}
                          {item.reason && (
                            <span className="text-xs text-secondary ml-1">{item.reason}</span>
                          )}
                        </td>
                        <td>
                          {hasDiagnostics ? (
                            <button
                              type="button"
                              className="btn btn-secondary btn-sm"
                              onClick={() =>
                                setExpandedSystemLogId(
                                  expandedSystemLogId === item.id ? null : item.id
                                )
                              }
                              aria-expanded={expandedSystemLogId === item.id}
                            >
                              {expandedSystemLogId === item.id ? "收起" : "诊断"}
                            </button>
                          ) : (
                            <span className="secondary small">—</span>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}

          {/* Expandable diagnostics viewer */}
          {expandedSystemLogId && (() => {
            const currentItem = systemItems.find(i => i.id === expandedSystemLogId);
            if (!currentItem || !currentItem.diagnostics) return null;
            return (
              <div
                className="subcard stack compact log-detail-card"
                role="region"
                aria-label={`诊断详情 · ${currentItem.event}`}
              >
                <div className="section-heading compact">
                  <span className="secondary small">
                    诊断信息 · {currentItem.event} · {formatLogTime(currentItem.timestamp)}
                  </span>
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    onClick={() => setExpandedSystemLogId(null)}
                  >
                    关闭
                  </button>
                </div>
                <pre className="log-error-body font-mono text-xs">
                  {typeof currentItem.diagnostics === "string"
                    ? currentItem.diagnostics
                    : JSON.stringify(currentItem.diagnostics, null, 2)}
                </pre>
              </div>
            );
          })()}
        </>
      )}
    </section>
  );
}
