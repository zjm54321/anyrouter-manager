import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CheckinPanel } from "./CheckinPanel";
import type { CheckinRecord, CheckinSettings } from "./CheckinPanel";

const defaultSettings: CheckinSettings = {
  enabled: true,
  time: "08:30",
  timezone: "Asia/Shanghai",
};

const makeRecord = (overrides: Partial<CheckinRecord> = {}): CheckinRecord => ({
  date: "2026-10-01",
  accountUserId: "10",
  account_user_id: "10",
  trigger: "manual",
  status: "success",
  startedAt: "2026-10-01T08:30:00Z",
  started_at: "2026-10-01T08:30:00Z",
  finishedAt: "2026-10-01T08:30:02Z",
  finished_at: "2026-10-01T08:30:02Z",
  code: "ok",
  ...overrides,
});

describe("CheckinPanel 独立签到面板", () => {
  it("正确展示今日尚未签到状态，并支持触发手动签到", () => {
    const onRun = vi.fn();
    const onUpdateSettings = vi.fn();

    render(
      <CheckinPanel
        settings={defaultSettings}
        today={null}
        history={[]}
        onRun={onRun}
        onUpdateSettings={onUpdateSettings}
      />
    );

    expect(screen.getByRole("heading", { name: "每日自动签到" })).toBeInTheDocument();
    expect(screen.getByText("当前周期尚未签到")).toBeInTheDocument();

    const runButton = screen.getByRole("button", { name: "立即签到" });
    expect(runButton).toBeEnabled();
    fireEvent.click(runButton);

    expect(onRun).toHaveBeenCalledWith({ confirmRetry: false });
  });

  it("今日签到成功后，签到按钮置灰并显示'今日已完成'", () => {
    const onRun = vi.fn();
    const onUpdateSettings = vi.fn();

    render(
      <CheckinPanel
        settings={defaultSettings}
        today={makeRecord({ status: "success" })}
        history={[]}
        onRun={onRun}
        onUpdateSettings={onUpdateSettings}
      />
    );

    expect(screen.getByText("今日已签到")).toBeInTheDocument();
    const runButton = screen.getByRole("button", { name: "今日已完成" });
    expect(runButton).toBeDisabled();
  });

  it("今日签到状态为 unknown 时，点击重试必须经过人工显式确认弹窗", () => {
    const onRun = vi.fn();
    const onUpdateSettings = vi.fn();

    render(
      <CheckinPanel
        settings={defaultSettings}
        today={makeRecord({ status: "unknown", code: "timeout" })}
        history={[]}
        onRun={onRun}
        onUpdateSettings={onUpdateSettings}
      />
    );

    expect(screen.getByText("状态待确认")).toBeInTheDocument();
    const retryButton = screen.getByRole("button", { name: "重试签到" });
    expect(retryButton).toBeEnabled();

    // 点击重试，不应立即调用 onRun，而应弹出确认弹窗
    fireEvent.click(retryButton);
    expect(onRun).not.toHaveBeenCalled();

    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "确认重试签到" })).toBeInTheDocument();
    expect(
      screen.getByText(/上一次签到状态未知.*再次签到可能触发重复请求/)
    ).toBeInTheDocument();

    // 取消弹窗
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(onRun).not.toHaveBeenCalled();

    // 再次点击并确认
    fireEvent.click(screen.getByRole("button", { name: "重试签到" }));
    fireEvent.click(screen.getByRole("button", { name: "确认继续重试" }));

    expect(onRun).toHaveBeenCalledWith({ confirmRetry: true });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("支持编辑签到时间并保存设置，dirty 检查生效", () => {
    const onRun = vi.fn();
    const onUpdateSettings = vi.fn();

    render(
      <CheckinPanel
        settings={defaultSettings}
        today={null}
        history={[]}
        onRun={onRun}
        onUpdateSettings={onUpdateSettings}
      />
    );

    const saveButton = screen.getByRole("button", { name: "保存配置" });
    // 未修改时应禁用保存按钮
    expect(saveButton).toBeDisabled();

    const timeInput = screen.getByLabelText("每日签到时间");
    fireEvent.change(timeInput, { target: { value: "09:15" } });

    expect(saveButton).toBeEnabled();
    fireEvent.click(saveButton);

    expect(onUpdateSettings).toHaveBeenCalledWith({
      enabled: true,
      time: "09:15",
      timezone: "Asia/Shanghai",
    });
  });

  it("当历史记录超过 10 条时，提供展开/收起较早记录功能", () => {
    const onRun = vi.fn();
    const onUpdateSettings = vi.fn();

    const history: CheckinRecord[] = Array.from({ length: 15 }, (_, i) =>
      makeRecord({
        date: `2026-09-${String(30 - i).padStart(2, "0")}`,
        code: `code-${i}`,
      })
    );

    render(
      <CheckinPanel
        settings={defaultSettings}
        today={null}
        history={history}
        onRun={onRun}
        onUpdateSettings={onUpdateSettings}
      />
    );

    // 默认展示 10 条
    const expandButton = screen.getByRole("button", { name: "查看较早记录（共 15 条）" });
    expect(expandButton).toBeInTheDocument();

    // 点击展开
    fireEvent.click(expandButton);
    expect(screen.getByRole("button", { name: "收起较早记录" })).toBeInTheDocument();

    // 点击收起
    fireEvent.click(screen.getByRole("button", { name: "收起较早记录" }));
    expect(screen.getByRole("button", { name: "查看较早记录（共 15 条）" })).toBeInTheDocument();
  });

  it("历史记录为空时展示暂无记录提示", () => {
    render(
      <CheckinPanel
        settings={defaultSettings}
        today={null}
        history={[]}
        onRun={vi.fn()}
        onUpdateSettings={vi.fn()}
      />
    );

    expect(screen.getByText("暂无签到历史记录")).toBeInTheDocument();
  });
});
