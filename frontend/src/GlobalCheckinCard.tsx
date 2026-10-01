import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent } from "react";
import type { GlobalCheckinConfig } from "./accountsTypes";
import { formatTimeInSlot } from "./accountsTypes";

interface GlobalCheckinCardProps {
  globalConfig: GlobalCheckinConfig;
  busy: boolean;
  onSave?: (cfg: GlobalCheckinConfig) => Promise<void> | void;
}

export function GlobalCheckinCard({ globalConfig, busy, onSave }: GlobalCheckinCardProps) {
  const enableCheckinId = useId();
  const startTimeId = useId();
  const intervalId = useId();

  const [checkinEnabled, setCheckinEnabled] = useState(globalConfig.enabled);
  const [startTime, setStartTime] = useState(globalConfig.startTime);
  const [intervalMinutes, setIntervalMinutes] = useState(globalConfig.intervalMinutes);
  const [configFeedback, setConfigFeedback] = useState("");
  const [isSavingConfig, setIsSavingConfig] = useState(false);

  const prevConfigRef = useRef(globalConfig);

  useEffect(() => {
    const prev = prevConfigRef.current;
    if (
      !prev ||
      prev.enabled !== globalConfig.enabled ||
      prev.startTime !== globalConfig.startTime ||
      prev.intervalMinutes !== globalConfig.intervalMinutes
    ) {
      setCheckinEnabled(globalConfig.enabled);
      setStartTime(globalConfig.startTime);
      setIntervalMinutes(globalConfig.intervalMinutes);
      prevConfigRef.current = globalConfig;
    }
  }, [globalConfig]);

  const isConfigDirty =
    checkinEnabled !== globalConfig.enabled ||
    startTime !== globalConfig.startTime ||
    intervalMinutes !== globalConfig.intervalMinutes;

  const isIntervalValid =
    Number.isInteger(intervalMinutes) && intervalMinutes >= 1 && intervalMinutes <= 1440;
  const isStartTimeValid = /^([01]\d|2[0-3]):[0-5]\d$/.test(startTime);

  async function handleSave(e: FormEvent) {
    e.preventDefault();
    if (!onSave || !isConfigDirty || !isIntervalValid || !isStartTimeValid) return;

    setIsSavingConfig(true);
    setConfigFeedback("");
    try {
      await onSave({
        enabled: checkinEnabled,
        startTime,
        intervalMinutes,
      });
      setConfigFeedback("配置已保存");
      prevConfigRef.current = {
        ...globalConfig,
        enabled: checkinEnabled,
        startTime,
        intervalMinutes,
      };
      setTimeout(() => setConfigFeedback(""), 3000);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : "保存全局签到配置失败";
      setConfigFeedback(msg);
    } finally {
      setIsSavingConfig(false);
    }
  }

  const isErrorFeedback =
    configFeedback.includes("失败") ||
    configFeedback.includes("溢出") ||
    configFeedback.includes("错误");

  return (
    <form className="card global-checkin-card stack" onSubmit={handleSave} aria-labelledby="global-checkin-heading">
      <div className="section-heading compact">
        <h3 id="global-checkin-heading">全局自动签到设置</h3>
        <span className="secondary small numeric">
          北京时间（Asia/Shanghai · 08:00 日切
          {globalConfig.cycleDate ? ` · ${globalConfig.cycleDate} 周期` : ""}）
        </span>
      </div>

      <div className="accounts-checkin-grid">
        <label className="checkbox-label" htmlFor={enableCheckinId}>
          <input
            id={enableCheckinId}
            type="checkbox"
            checked={checkinEnabled}
            onChange={e => setCheckinEnabled(e.target.checked)}
            disabled={busy || isSavingConfig}
          />
          <span>启用每日自动签到</span>
        </label>

        <div className="field">
          <label htmlFor={startTimeId}>起始签到时间</label>
          <input
            id={startTimeId}
            type="time"
            step="60"
            required
            value={startTime}
            onChange={e => setStartTime(e.target.value)}
            disabled={busy || isSavingConfig}
          />
        </div>

        <div className="field">
          <label htmlFor={intervalId}>账号签到间隔（分钟）</label>
          <input
            id={intervalId}
            type="number"
            min={1}
            max={1440}
            required
            value={intervalMinutes}
            onChange={e => setIntervalMinutes(parseInt(e.target.value, 10) || 1)}
            disabled={busy || isSavingConfig}
          />
        </div>
      </div>

      <p className="secondary small numeric">
        提示：按添加顺序固定执行 slot（例如 {startTime}、
        {formatTimeInSlot(startTime, intervalMinutes, 1)}、
        {formatTimeInSlot(startTime, intervalMinutes, 2)}
        …），非完成时间后推。单日跨度超过 24 小时将触发溢出拦截。
      </p>

      <div className="actions">
        <button
          type="submit"
          className="primary"
          disabled={
            !isConfigDirty ||
            !isIntervalValid ||
            !isStartTimeValid ||
            isSavingConfig ||
            busy
          }
        >
          {isSavingConfig ? "正在保存…" : "保存全局签到配置"}
        </button>
        {configFeedback && (
          <span
            className={`feedback ${isErrorFeedback ? "error-text" : ""}`}
            role={isErrorFeedback ? "alert" : "status"}
          >
            {configFeedback}
          </span>
        )}
      </div>
    </form>
  );
}
