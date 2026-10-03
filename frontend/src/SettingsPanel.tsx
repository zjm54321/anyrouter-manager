import { useEffect, useState } from "react";
import {
  AlertTriangle,
  Check,
  Copy,
  FileText,
  Globe,
  Laptop,
  Moon,
  Palette,
  Sliders,
  Sun,
  Trash2,
} from "lucide-react";
import type { GlobalCheckinConfig } from "./accountsTypes";
import type { GatewayResponsesMode, GatewaySettings, LogLevel, LogSettings } from "./api";
import { GlobalCheckinCard } from "./GlobalCheckinCard";

interface SettingsPanelProps {
  globalConfig: GlobalCheckinConfig;
  busy: boolean;
  onSaveGlobalCheckin: (config: {
    enabled: boolean;
    startTime: string;
    intervalMinutes: number;
  }) => Promise<void>;
  logSettings?: LogSettings | null;
  onSaveLogSettings?: (settings: LogSettings) => Promise<void>;
  gatewaySettings?: GatewaySettings | null;
  gatewaySettingsError?: string | null;
  onSaveGatewaySettings?: (settings: GatewaySettings) => Promise<GatewaySettings>;
  onRetryGatewaySettings?: () => Promise<void> | void;
  onClearSystemLogs?: () => Promise<void>;
  onClearRequestLogs?: () => Promise<void>;
}

export function SettingsPanel({
  globalConfig,
  busy,
  onSaveGlobalCheckin,
  logSettings,
  onSaveLogSettings,
  gatewaySettings,
  gatewaySettingsError,
  onSaveGatewaySettings,
  onRetryGatewaySettings,
  onClearSystemLogs,
  onClearRequestLogs,
}: SettingsPanelProps) {
  // Theme state
  const [theme, setTheme] = useState<"system" | "light" | "dark">(() => {
    try {
      const saved = localStorage.getItem("theme");
      if (saved === "light" || saved === "dark" || saved === "system") {
        return saved;
      }
    } catch {
      // localStorage may fail in restricted context
    }
    return "system";
  });

  useEffect(() => {
    try {
      localStorage.setItem("theme", theme);
    } catch {
      // ignore
    }
    if (theme === "system") {
      document.documentElement.removeAttribute("data-theme");
    } else {
      document.documentElement.setAttribute("data-theme", theme);
    }
  }, [theme]);

  // Dynamic Gateway URL
  const gatewayUrl = typeof window !== "undefined"
    ? `${window.location.origin}/v1`
    : "http://127.0.0.1:18880/v1";

  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(gatewayUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // fallback
      const textArea = document.createElement("textarea");
      textArea.value = gatewayUrl;
      textArea.style.position = "fixed";
      textArea.style.opacity = "0";
      document.body.appendChild(textArea);
      textArea.focus();
      textArea.select();
      try {
        document.execCommand("copy");
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } catch {
        // failed
      }
      document.body.removeChild(textArea);
    }
  }

  // Gateway Responses Mode state
  const [selectedMode, setSelectedMode] = useState<GatewayResponsesMode | null>(
    gatewaySettings?.responses_mode ?? null
  );
  const [gatewaySaving, setGatewaySaving] = useState(false);
  const [gatewayFeedback, setGatewayFeedback] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  useEffect(() => {
    if (gatewaySettings) {
      setSelectedMode(gatewaySettings.responses_mode);
    }
  }, [gatewaySettings]);

  async function handleSaveGatewaySettings(e: React.FormEvent) {
    e.preventDefault();
    if (!onSaveGatewaySettings || !selectedMode) return;
    setGatewaySaving(true);
    setGatewayFeedback(null);
    try {
      const confirmed = await onSaveGatewaySettings({ responses_mode: selectedMode });
      if (confirmed && confirmed.responses_mode === selectedMode) {
        setGatewayFeedback({ type: "success", message: "网关设置已保存" });
      } else {
        throw new Error("服务端确认的设置格式无效或已被更改");
      }
    } catch (err) {
      setGatewayFeedback({
        type: "error",
        message: err instanceof Error ? `保存失败：${err.message}` : "保存网关设置失败",
      });
      // Retain last confirmed server mode / not optimistic success
      if (gatewaySettings) {
        setSelectedMode(gatewaySettings.responses_mode);
      }
    } finally {
      setGatewaySaving(false);
    }
  }

  // Log Settings form state
  const [level, setLevel] = useState<LogLevel>(logSettings?.level || "info");
  const [systemRetention, setSystemRetention] = useState<number>(
    logSettings?.system_retention_days ?? 7
  );
  const [requestRetention, setRequestRetention] = useState<number>(
    logSettings?.request_retention_days ?? 7
  );
  const [logSettingsSaving, setLogSettingsSaving] = useState(false);
  const [logSettingsFeedback, setLogSettingsFeedback] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  // Sync incoming log settings
  useEffect(() => {
    if (logSettings) {
      setLevel(logSettings.level);
      setSystemRetention(logSettings.system_retention_days);
      setRequestRetention(logSettings.request_retention_days);
    }
  }, [logSettings]);

  async function handleSaveLogSettings(e: React.FormEvent) {
    e.preventDefault();
    if (!onSaveLogSettings) return;
    setLogSettingsSaving(true);
    setLogSettingsFeedback(null);
    try {
      await onSaveLogSettings({
        level,
        system_retention_days: Number(systemRetention),
        request_retention_days: Number(requestRetention),
      });
      setLogSettingsFeedback({ type: "success", message: "日志设置已保存" });
    } catch (err: any) {
      setLogSettingsFeedback({
        type: "error",
        message: err?.message || "保存日志设置失败",
      });
    } finally {
      setLogSettingsSaving(false);
    }
  }

  // Clear confirmation states
  const [confirmClearType, setConfirmClearType] = useState<"system" | "request" | null>(null);
  const [clearing, setClearing] = useState(false);
  const [clearFeedback, setClearFeedback] = useState<string | null>(null);

  async function executeClear() {
    if (!confirmClearType) return;
    setClearing(true);
    try {
      if (confirmClearType === "system" && onClearSystemLogs) {
        await onClearSystemLogs();
        setClearFeedback("系统日志已清空");
      } else if (confirmClearType === "request" && onClearRequestLogs) {
        await onClearRequestLogs();
        setClearFeedback("网关请求日志已清空");
      }
      setConfirmClearType(null);
      setTimeout(() => setClearFeedback(null), 3000);
    } catch (err: any) {
      setClearFeedback(`清空失败: ${err?.message || "未知错误"}`);
    } finally {
      setClearing(false);
    }
  }

  return (
    <div className="settings-container stack">
      {/* 1. Global Checkin Configuration */}
      <GlobalCheckinCard
        globalConfig={globalConfig}
        busy={busy}
        onSave={onSaveGlobalCheckin}
      />

      {/* 2. Gateway URL & Client Guidance */}
      <section className="card stack" aria-labelledby="gateway-card-title">
        <div className="section-heading">
          <div>
            <h3 id="gateway-card-title" className="text-base font-semibold">本地网关接入地址</h3>
          </div>
        </div>

        <div className="gateway-box">
          <div className="gateway-endpoint-row">
            <span className="gateway-badge">OpenAI 兼容端点</span>
            <code className="gateway-url font-mono">{gatewayUrl}</code>
            <button
              type="button"
              className="btn btn-secondary btn-sm copy-btn"
              onClick={handleCopy}
            >
              {copied ? <Check size={14} className="text-teal" /> : <Copy size={14} />}
              <span>{copied ? "已复制" : "复制地址"}</span>
            </button>
          </div>
          {copied && (
            <span className="feedback" role="status">已复制网关地址</span>
          )}
        </div>

        <div className="divider" style={{ margin: "16px 0 12px 0" }} />

        {/* Responses 转发模式 */}
        <div className="gateway-mode-section stack compact" aria-labelledby="gateway-mode-title">
          <div className="section-heading compact">
            <div>
              <h4 id="gateway-mode-title" className="text-sm font-semibold">Responses 转发模式</h4>
              <p className="secondary text-xs">
                仅针对 POST /v1/responses 接口生效；其他通用路由原样透传。
              </p>
            </div>
          </div>

          {gatewaySettingsError && (
            <div className="alert alert-error" role="alert" style={{ fontSize: "13px", padding: "10px 14px" }}>
              <div className="cluster" style={{ justifyContent: "space-between", width: "100%" }}>
                <span>{gatewaySettingsError}</span>
                {onRetryGatewaySettings && (
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    onClick={() => onRetryGatewaySettings()}
                  >
                    重试
                  </button>
                )}
              </div>
            </div>
          )}

          {!gatewaySettingsError && gatewaySettings === null && (
            <div className="secondary text-sm p-3">正在获取网关转发模式设置…</div>
          )}

          {gatewaySettings && (
            <form onSubmit={handleSaveGatewaySettings} className="stack compact">
              <div className="gateway-mode-grid" role="radiogroup" aria-labelledby="gateway-mode-title">
                <label className={`gateway-mode-card ${selectedMode === "pass" ? "active" : ""}`}>
                  <input
                    type="radio"
                    name="responses_mode"
                    value="pass"
                    checked={selectedMode === "pass"}
                    onChange={() => setSelectedMode("pass")}
                    disabled={busy || gatewaySaving}
                  />
                  <div className="gateway-mode-card-body">
                    <span className="gateway-mode-card-title">全部透传（默认）</span>
                    <span className="gateway-mode-card-desc secondary text-xs">
                      不修改请求体（Body）。网关认证头正常处理，通用路径原样转发。
                    </span>
                  </div>
                </label>

                <label className={`gateway-mode-card ${selectedMode === "adapt" ? "active" : ""}`}>
                  <input
                    type="radio"
                    name="responses_mode"
                    value="adapt"
                    checked={selectedMode === "adapt"}
                    onChange={() => setSelectedMode("adapt")}
                    disabled={busy || gatewaySaving}
                  />
                  <div className="gateway-mode-card-body">
                    <span className="gateway-mode-card-title">兼容适配（实验性）</span>
                    <span className="gateway-mode-card-desc secondary text-xs">
                      仅针对缺少 prompt_cache_key 的请求补齐缓存标识，其余入参格式原样保留。
                    </span>
                  </div>
                </label>

                <label className={`gateway-mode-card ${selectedMode === "auto" ? "active" : ""}`}>
                  <input
                    type="radio"
                    name="responses_mode"
                    value="auto"
                    checked={selectedMode === "auto"}
                    onChange={() => setSelectedMode("auto")}
                    disabled={busy || gatewaySaving}
                  />
                  <div className="gateway-mode-card-body">
                    <span className="gateway-mode-card-title">自动识别（实验性）</span>
                    <span className="gateway-mode-card-desc secondary text-xs">
                      按客户端特征识别：已知 OpenCode / Codex 客户端直接透传，其余缺失时按需补充。
                    </span>
                  </div>
                </label>
              </div>

              <div className="notice-box text-xs secondary" style={{ margin: "4px 0" }}>
                <span>实验性：仅补充缺失的缓存标识；通用 UUID 与更多客户端场景的上游兼容性尚未验证。</span>
              </div>

              <div className="cluster" style={{ justifyContent: "flex-end", gap: "10px", marginTop: "4px" }}>
                {gatewayFeedback && (
                  <span
                    className={`text-xs ${gatewayFeedback.type === "success" ? "text-teal" : "error-text"}`}
                    role="status"
                  >
                    {gatewayFeedback.message}
                  </span>
                )}
                <button
                  type="submit"
                  className="btn btn-primary btn-sm"
                  disabled={
                    busy ||
                    gatewaySaving ||
                    selectedMode === null ||
                    selectedMode === gatewaySettings.responses_mode
                  }
                >
                  {gatewaySaving ? "保存中…" : "保存设置"}
                </button>
              </div>
            </form>
          )}
        </div>
      </section>

      {/* 3. Theme Preferences */}
      <section className="card stack" aria-labelledby="theme-card-title">
        <div className="section-heading">
          <div>
            <h3 id="theme-card-title" className="text-base font-semibold">主题外观偏好</h3>
          </div>
        </div>

        <div className="theme-options-grid">
          <label className={`theme-card ${theme === "system" ? "active" : ""}`}>
            <input
              type="radio"
              name="theme"
              value="system"
              checked={theme === "system"}
              onChange={() => setTheme("system")}
            />
            <div className="theme-card-body">
              <span className="theme-card-title">
                <Laptop size={16} />
                <span>跟随系统</span>
              </span>
            </div>
          </label>

          <label className={`theme-card ${theme === "dark" ? "active" : ""}`}>
            <input
              type="radio"
              name="theme"
              value="dark"
              checked={theme === "dark"}
              onChange={() => setTheme("dark")}
            />
            <div className="theme-card-body">
              <span className="theme-card-title">
                <Moon size={16} />
                <span>石墨暖炭（深色）</span>
              </span>
            </div>
          </label>

          <label className={`theme-card ${theme === "light" ? "active" : ""}`}>
            <input
              type="radio"
              name="theme"
              value="light"
              checked={theme === "light"}
              onChange={() => setTheme("light")}
            />
            <div className="theme-card-body">
              <span className="theme-card-title">
                <Sun size={16} />
                <span>明亮灰度（浅色）</span>
              </span>
            </div>
          </label>
        </div>
      </section>

      {/* 4. Logging & Retention Preferences (Wired to /api/log-settings) */}
      <section className="card stack" aria-labelledby="log-settings-title">
        <div className="section-heading">
          <div>
            <h3 id="log-settings-title" className="text-base font-semibold">日志记录与保留设置</h3>
          </div>
        </div>

        {logSettingsFeedback && (
          <div
            className={`alert ${logSettingsFeedback.type === "success" ? "alert-success" : "alert-error"}`}
            role="alert"
          >
            <span>{logSettingsFeedback.message}</span>
          </div>
        )}

        <form onSubmit={handleSaveLogSettings} className="stack">
          <div className="form-grid">
            <div className="field">
              <label htmlFor="log-level-select" className="field-label">日志级别</label>
              <div className="select-wrapper">
                <select
                  id="log-level-select"
                  className="select-input"
                  value={level}
                  onChange={e => setLevel(e.target.value as LogLevel)}
                  disabled={busy || logSettingsSaving}
                  aria-label="日志记录等级"
                >
                  <option value="error">ERROR</option>
                  <option value="warn">WARN</option>
                  <option value="info">INFO</option>
                  <option value="debug">DEBUG</option>
                  <option value="trace">TRACE</option>
                </select>
              </div>
            </div>

            <div className="field">
              <label htmlFor="system-retention-input" className="field-label">系统日志保留天数 (1-90)</label>
              <input
                id="system-retention-input"
                type="number"
                min="1"
                max="90"
                className="input-text"
                value={systemRetention}
                onChange={e => setSystemRetention(Math.max(1, Math.min(90, parseInt(e.target.value, 10) || 1)))}
                disabled={busy || logSettingsSaving}
                required
              />
            </div>

            <div className="field">
              <label htmlFor="request-retention-input" className="field-label">网关请求日志保留天数 (1-90)</label>
              <input
                id="request-retention-input"
                type="number"
                min="1"
                max="90"
                className="input-text"
                value={requestRetention}
                onChange={e => setRequestRetention(Math.max(1, Math.min(90, parseInt(e.target.value, 10) || 1)))}
                disabled={busy || logSettingsSaving}
                required
              />
            </div>
          </div>

          <div className="form-actions flex justify-between items-center" style={{ marginTop: "1rem" }}>
            <button
              type="submit"
              className="btn btn-primary btn-sm"
              disabled={busy || logSettingsSaving}
            >
              {logSettingsSaving ? "保存中…" : "保存日志配置"}
            </button>
          </div>
        </form>

        {/* 5. Separate Clear Confirmations */}
        <div className="danger-zone-card stack compact" style={{ marginTop: "1.5rem", borderTop: "1px solid var(--border-color)", paddingTop: "1rem" }}>
          <h4 className="text-sm font-semibold text-secondary">日志管理操作</h4>
          {clearFeedback && (
            <div className="alert alert-info text-xs" role="status">
              <span>{clearFeedback}</span>
            </div>
          )}
          <div className="flex gap-3 flex-wrap">
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => setConfirmClearType("system")}
              disabled={busy || clearing}
            >
              <Trash2 size={14} />
              <span>清空系统运行日志</span>
            </button>
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => setConfirmClearType("request")}
              disabled={busy || clearing}
            >
              <Trash2 size={14} />
              <span>清空网关请求日志</span>
            </button>
          </div>
        </div>

        {/* Confirmation Modal */}
        {confirmClearType && (
          <div className="dialog-backdrop" role="dialog" aria-modal="true" aria-labelledby="clear-confirm-title">
            <div className="dialog-card stack">
              <div className="dialog-header">
                <div className="flex items-center gap-2 text-warn">
                  <AlertTriangle size={18} />
                  <h3 id="clear-confirm-title" className="dialog-title">确认清空日志</h3>
                </div>
              </div>
              <div className="dialog-body">
                <p className="secondary small">
                  此操作将立即清空当前所有的{confirmClearType === "system" ? "系统运行日志" : "网关请求日志"}，不可恢复。
                </p>
              </div>
              <div className="dialog-footer flex justify-end gap-2">
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  onClick={() => setConfirmClearType(null)}
                  disabled={clearing}
                >
                  取消
                </button>
                <button
                  type="button"
                  className="btn btn-error btn-sm"
                  onClick={executeClear}
                  disabled={clearing}
                >
                  {clearing ? "清空中…" : "确认清空"}
                </button>
              </div>
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
