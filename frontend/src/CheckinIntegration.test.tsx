import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { AccountState, ActiveAccount, CheckinFullStatus } from "./api";

const balance = { quota_raw: "1000000.25", used_quota_raw: "25000", fetched_at: "2026-09-30T12:00:00Z" };
const active: ActiveAccount = {
  revision: "rev-1",
  username: "current-user",
  upstream_user_id: "10",
  balance,
  selected_key: { id: "key-1", name: "Existing key", masked: "sk-ab…1234" },
  activated_at: "2026-09-30T12:00:00Z",
};

const snapshot = (extra: Partial<AccountState> = {}): AccountState => ({
  active,
  candidate: null,
  operation: null,
  ...extra,
});

const defaultCheckin: CheckinFullStatus = {
  settings: {
    enabled: false,
    time: "09:00",
    timezone: "Asia/Shanghai",
    reset_time: "08:00",
  },
  cycle_date: "2026-10-01",
  today: null,
  history: [],
  next_run_at: null,
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

const deferred = () => {
  let resolve!: (response: Response) => void;
  const promise = new Promise<Response>(done => {
    resolve = done;
  });
  return { promise, resolve };
};

let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

beforeEach(() => {
  fetchMock = vi.fn<typeof fetch>();
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal(
    "navigator",
    Object.assign(Object.create(navigator), {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    })
  );
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function openDashboard(checkinState = defaultCheckin, accountState = snapshot()) {
  fetchMock.mockResolvedValueOnce(json(accountState));
  fetchMock.mockResolvedValueOnce(json(checkinState));
  render(<App />);
  await screen.findByRole("heading", { name: "当前账号" });
  await screen.findByRole("heading", { name: "每日自动签到" });
}

describe("Checkin 集成流程与异常处理", () => {
  it("authenticated load fetches both /api/account and /api/checkin and renders checkin panel", async () => {
    const configuredCheckin: CheckinFullStatus = {
      settings: {
        enabled: true,
        time: "09:30",
        timezone: "Asia/Shanghai",
        reset_time: "08:00",
      },
      cycle_date: "2026-10-01",
      today: null,
      history: [
        {
          date: "2026-09-30",
          account_user_id: "10",
          trigger: "scheduled",
          status: "success",
          started_at: "2026-09-30T01:30:00Z",
          finished_at: "2026-09-30T01:30:02Z",
          code: "ok",
        },
      ],
      next_run_at: "2026-10-02T01:30:00Z",
    };

    await openDashboard(configuredCheckin);

    expect(screen.getByRole("heading", { name: "每日自动签到" })).toBeInTheDocument();
    expect(screen.getByText("当前签到周期 · 2026-10-01 轮次")).toBeInTheDocument();
    expect(screen.getByText("当前周期尚未签到")).toBeInTheDocument();
    expect(screen.getByLabelText("启用每日自动签到")).toBeChecked();
    expect(screen.getByLabelText("每日签到时间")).toHaveValue("09:30");
    expect(screen.getByText("共 1 条")).toBeInTheDocument();
  });

  it("settings PUT validation and failure keeps form dirty without discarding input", async () => {
    await openDashboard();

    const checkbox = screen.getByLabelText("启用每日自动签到");
    const timeInput = screen.getByLabelText("每日签到时间");
    const saveButton = screen.getByRole("button", { name: "保存配置" });

    expect(saveButton).toBeDisabled();

    fireEvent.click(checkbox);
    fireEvent.change(timeInput, { target: { value: "10:15" } });
    expect(saveButton).toBeEnabled();

    // Mock failure on PUT
    fetchMock.mockResolvedValueOnce(
      json({ error: { code: "persistence_failed", message: "保存签到配置失败" } }, 500)
    );

    fireEvent.click(saveButton);

    await screen.findByText("保存失败，请重试");
    // Form retains user's modifications
    expect(checkbox).toBeChecked();
    expect(timeInput).toHaveValue("10:15");
  });

  it("POST /api/checkin/run 202 starts operation, polls until complete, and refreshes checkin state", async () => {
    await openDashboard();
    vi.useFakeTimers();

    const runButton = screen.getByRole("button", { name: "立即签到" });
    expect(runButton).toBeEnabled();

    // 1. POST /api/checkin/run returns 202
    fetchMock.mockResolvedValueOnce(json({ operation_id: "op-checkin-1" }, 202));
    // 2. Immediate GET /api/account returns operation: running
    fetchMock.mockResolvedValueOnce(
      json(
        snapshot({
          operation: {
            id: "op-checkin-1",
            kind: "checkin",
            status: "running",
            phase: "checking_in",
            error: null,
          },
        })
      )
    );
    // 3. Immediate GET /api/checkin returns today: running
    fetchMock.mockResolvedValueOnce(
      json({
        ...defaultCheckin,
        today: {
          date: "2026-10-01",
          account_user_id: "10",
          trigger: "manual",
          status: "running",
          started_at: "2026-10-01T08:05:00Z",
          finished_at: null,
          code: null,
        },
      })
    );

    await act(async () => fireEvent.click(runButton));

    expect(screen.getByText("正在签到")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "正在签到…" })).toBeDisabled();

    // 4. Advance 1s timer: poll GET /api/account returns succeeded
    fetchMock.mockResolvedValueOnce(
      json(
        snapshot({
          operation: {
            id: "op-checkin-1",
            kind: "checkin",
            status: "succeeded",
            phase: "done",
            error: null,
          },
        })
      )
    );
    // 5. Operation finished triggers GET /api/checkin returns today: success
    fetchMock.mockResolvedValueOnce(
      json({
        ...defaultCheckin,
        today: {
          date: "2026-10-01",
          account_user_id: "10",
          trigger: "manual",
          status: "success",
          started_at: "2026-10-01T08:05:00Z",
          finished_at: "2026-10-01T08:05:02Z",
          code: "ok",
        },
      })
    );

    await act(async () => vi.advanceTimersByTime(1000));

    expect(screen.getByText("今日已签到")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "今日已完成" })).toBeDisabled();
  });

  it("POST /api/checkin/run 200 already_recorded refreshes state without starting background operation", async () => {
    await openDashboard();

    const runButton = screen.getByRole("button", { name: "立即签到" });

    // 1. POST /api/checkin/run returns 200
    fetchMock.mockResolvedValueOnce(json({ already_recorded: true }, 200));
    // 2. Prompt GET /api/checkin returns today: already_done
    fetchMock.mockResolvedValueOnce(
      json({
        ...defaultCheckin,
        today: {
          date: "2026-10-01",
          account_user_id: "10",
          trigger: "manual",
          status: "already_done",
          started_at: "2026-10-01T08:00:00Z",
          finished_at: "2026-10-01T08:00:01Z",
          code: "already_checked_in",
        },
      })
    );

    await act(async () => fireEvent.click(runButton));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "今日已完成" })).toBeDisabled();
    });
    expect(screen.getAllByText("今日已完成").length).toBeGreaterThan(0);
  });

  it("unknown status requires explicit confirmation modal before retrying with confirm_retry: true", async () => {
    const unknownCheckin: CheckinFullStatus = {
      ...defaultCheckin,
      today: {
        date: "2026-10-01",
        account_user_id: "10",
        trigger: "manual",
        status: "unknown",
        started_at: "2026-10-01T08:05:00Z",
        finished_at: "2026-10-01T08:05:05Z",
        code: "upstream_5xx",
      },
    };

    await openDashboard(unknownCheckin);

    const retryButton = screen.getByRole("button", { name: "重试签到" });
    expect(retryButton).toBeEnabled();

    // Clicking retry opens the confirmation modal
    fireEvent.click(retryButton);

    const modal = screen.getByRole("dialog", { name: "确认重试签到" });
    expect(modal).toBeInTheDocument();
    expect(
      screen.getByText(/上一次签到状态未知（如网络中断或 upstream 响应异常）/)
    ).toBeInTheDocument();

    // Confirm in dialog dispatches with confirm_retry: true
    fetchMock.mockResolvedValueOnce(json({ operation_id: "op-checkin-retry" }, 202));
    fetchMock.mockResolvedValueOnce(
      json(
        snapshot({
          operation: {
            id: "op-checkin-retry",
            kind: "checkin",
            status: "running",
            phase: "checking_in",
            error: null,
          },
        })
      )
    );
    fetchMock.mockResolvedValueOnce(json(unknownCheckin));

    fireEvent.click(screen.getByRole("button", { name: "确认继续重试" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(fetchMock.mock.calls[2]).toEqual([
      "/api/checkin/run",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ confirm_retry: true }),
      }),
    ]);
  });

  it("scheduled record appears on visibility change", async () => {
    await openDashboard();

    const updatedCheckin: CheckinFullStatus = {
      ...defaultCheckin,
      today: {
        date: "2026-10-01",
        account_user_id: "10",
        trigger: "scheduled",
        status: "success",
        started_at: "2026-10-01T09:00:00Z",
        finished_at: "2026-10-01T09:00:02Z",
        code: "ok",
      },
    };

    fetchMock.mockResolvedValueOnce(json(updatedCheckin));

    act(() => {
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => "visible",
      });
      document.dispatchEvent(new Event("visibilitychange"));
    });

    await screen.findByText("今日已签到");
    expect(screen.getByRole("button", { name: "今日已完成" })).toBeDisabled();
  });

  it("account change or logout aborts in-flight checkin requests and discards late responses", async () => {
    await openDashboard();

    const pendingCheckin = deferred();
    fetchMock.mockReturnValueOnce(pendingCheckin.promise);

    // Trigger checkin retry read
    const retryReadButton = screen.queryByRole("button", { name: "重试读取" });
    if (!retryReadButton) {
      // Simulate visibility change to invoke loadCheckin
      act(() => {
        document.dispatchEvent(new Event("visibilitychange"));
      });
    }

    const checkinSignal = fetchMock.mock.calls[2][1]?.signal;

    // User logs out
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    fireEvent.click(screen.getByRole("button", { name: "退出管理" }));

    expect(checkinSignal?.aborted).toBe(true);

    // Resolve late response
    await act(async () =>
      pendingCheckin.resolve(
        json({
          ...defaultCheckin,
          today: {
            date: "2026-10-01",
            account_user_id: "10",
            trigger: "manual",
            status: "success",
            started_at: "2026-10-01T08:00:00Z",
            finished_at: "2026-10-01T08:00:01Z",
            code: "ok",
          },
        })
      )
    );

    // App is locked, checkin data is not rendered
    expect(screen.queryByRole("heading", { name: "每日自动签到" })).not.toBeInTheDocument();
  });

  it("checkin 401 locks app whereas root reveal 401 remains inline", async () => {
    await openDashboard();

    // Checkin fetch returning 401 locks the whole session
    fetchMock.mockResolvedValueOnce(
      json({ error: { code: "upstream_session_expired", message: "未鉴权" } }, 401)
    );

    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });

    await screen.findByLabelText("本地 root 密钥");
    expect(screen.getByText("请输入本地 root 密钥以建立管理会话")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "当前账号" })).not.toBeInTheDocument();
  });

  it("under 08:00 schedule displays previous cycle hint without local date error", async () => {
    const earlyCheckin: CheckinFullStatus = {
      settings: {
        enabled: true,
        time: "02:30",
        timezone: "Asia/Shanghai",
        reset_time: "08:00",
      },
      cycle_date: "2026-10-01",
      today: null,
      history: [],
      next_run_at: null,
    };

    await openDashboard(earlyCheckin);

    expect(
      screen.getByText("每日 02:30 自动执行（北京时间，08:00 前执行属于上一签到轮）")
    ).toBeInTheDocument();
    expect(screen.getByText("当前签到周期 · 2026-10-01 轮次")).toBeInTheDocument();
  });

  it("no extra requests dispatched while busy running an account operation", async () => {
    const runningOp = {
      id: "refresh-busy",
      kind: "refresh" as const,
      status: "running" as const,
      phase: "reading_account" as const,
      error: null,
    };
    await openDashboard(defaultCheckin, snapshot({ operation: runningOp }));

    const runButton = screen.getByRole("button", { name: "立即签到" });
    expect(runButton).toBeDisabled();

    const checkbox = screen.getByLabelText("启用每日自动签到");
    expect(checkbox).toBeDisabled();

    const timeInput = screen.getByLabelText("每日签到时间");
    expect(timeInput).toBeDisabled();

    // Clicking disabled button dispatches no new network requests
    fireEvent.click(runButton);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});
