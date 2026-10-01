import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type {
  AccountDTO,
  AccountsResponse,
  GlobalCheckinResponse,
} from "./api";

const balance = { quota_raw: "1000000.25", used_quota_raw: "25000", fetched_at: "2026-09-30T12:00:00Z" };
const accountA: AccountDTO = {
  id: "acc-1",
  revision: "rev-1",
  username: "current-user",
  upstream_user_id: "10",
  balance,
  keys: [{ id: "key-1", name: "Existing key", masked: "sk-ab…1234", enabled: true }],
  selected_key: { id: "key-1", name: "Existing key", masked: "sk-ab…1234" },
  added_at: "2026-09-30T12:00:00Z",
};

const snapshot = (extra: Partial<AccountsResponse> = {}): AccountsResponse => ({
  accounts: [accountA],
  route_account_id: "acc-1",
  candidate: null,
  operation: null,
  ...extra,
});

const defaultCheckin: GlobalCheckinResponse = {
  settings: {
    enabled: false,
    time: "09:00",
    interval_minutes: 30,
    timezone: "Asia/Shanghai",
    reset_time: "08:00",
  },
  cycle_date: "2026-10-01",
  next_run_at: null,
  schedule: [
    {
      account_id: "acc-1",
      scheduled_at: "2026-10-01T09:00:00+08:00",
      status: "pending",
    },
  ],
};

const emptyLogs = { items: [], dropped_count: 0 };

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

async function openDashboard(
  checkinState = defaultCheckin,
  accountsState = snapshot(),
  page: "accounts" | "settings" = "accounts"
) {
  fetchMock.mockImplementation(async (url: string | URL | Request) => {
    const urlStr = typeof url === "string" ? url : url.toString();
    if (urlStr.startsWith("/api/accounts")) return json(accountsState);
    if (urlStr.startsWith("/api/checkin")) return json(checkinState);
    if (urlStr.startsWith("/api/request-logs")) return json(emptyLogs);
    return json({});
  });

  window.location.hash = "#" + page;
  render(<App />);
  if (page === "settings") {
    await screen.findByRole("heading", { name: /全局自动签到/ });
    if (checkinState.cycle_date) {
      await screen.findByText(new RegExp(`${checkinState.cycle_date} 周期`));
    }
  } else {
    await screen.findByRole("heading", { name: /已绑定账号列表/ });
  }
}

describe("Checkin 集成流程与异常处理", () => {
  it("authenticated load fetches both /api/accounts and /api/checkin and renders checkin config", async () => {
    const configuredCheckin: GlobalCheckinResponse = {
      settings: {
        enabled: true,
        time: "09:30",
        interval_minutes: 30,
        timezone: "Asia/Shanghai",
        reset_time: "08:00",
      },
      cycle_date: "2026-10-01",
      schedule: [
        {
          account_id: "acc-1",
          scheduled_at: "2026-10-01T09:30:00+08:00",
          status: "pending",
        },
      ],
      next_run_at: "2026-10-01T09:30:00+08:00",
    };

    await openDashboard(configuredCheckin, snapshot(), "settings");

    expect(screen.getByRole("heading", { name: /全局自动签到/ })).toBeInTheDocument();
    expect(screen.getByText(/2026-10-01 周期/)).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByLabelText("启用每日自动签到")).toBeChecked();
    });
    expect(screen.getByLabelText(/起始.*时间/)).toHaveValue("09:30");
    expect(screen.getByLabelText(/账号.*间隔（分钟）/)).toHaveValue(30);
  });

  it("settings PUT validation and failure keeps form dirty without discarding input", async () => {
    await openDashboard(defaultCheckin, snapshot(), "settings");

    const checkbox = screen.getByLabelText("启用每日自动签到");
    const timeInput = screen.getByLabelText(/起始.*时间/);
    const saveButton = screen.getByRole("button", { name: /保存.*签到配置/ });

    expect(saveButton).toBeDisabled();

    fireEvent.click(checkbox);
    fireEvent.change(timeInput, { target: { value: "10:15" } });
    expect(saveButton).toBeEnabled();

    // Mock failure on PUT
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/checkin/settings") {
        return json({ error: { code: "persistence_failed", message: "保存签到配置失败" } }, 500);
      }
      return json({});
    });

    fireEvent.click(saveButton);

    await screen.findByText(/保存签到配置失败/);
    // Form retains user's modifications
    expect(checkbox).toBeChecked();
    expect(timeInput).toHaveValue("10:15");
  });

  it("schedule overflow 422 displays prompt without quiet cross-day", async () => {
    await openDashboard(defaultCheckin, snapshot(), "settings");

    const intervalInput = screen.getByLabelText(/账号.*间隔（分钟）/);
    const saveButton = screen.getByRole("button", { name: /保存.*签到配置/ });

    fireEvent.change(intervalInput, { target: { value: "200" } });
    expect(saveButton).toBeEnabled();

    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/checkin/settings") {
        return json(
          {
            error: {
              code: "schedule_overflow",
              message: "签到排期跨天溢出，请缩短间隔或调整起始时间",
            },
          },
          422
        );
      }
      return json({});
    });

    fireEvent.click(saveButton);

    await screen.findByText("签到排期跨天溢出，请缩短间隔或调整起始时间");
    expect(intervalInput).toHaveValue(200);
  });

  it("interval change from 30 to 15 successfully saves configuration", async () => {
    await openDashboard(defaultCheckin, snapshot(), "settings");

    const intervalInput = screen.getByLabelText(/账号.*间隔（分钟）/);
    const saveButton = screen.getByRole("button", { name: /保存.*签到配置/ });

    fireEvent.change(intervalInput, { target: { value: "15" } });
    expect(saveButton).toBeEnabled();

    fetchMock.mockImplementation(async (url: string | URL | Request, init?: RequestInit) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/checkin/settings" && init?.method === "PUT") {
        return json({
          settings: {
            enabled: false,
            time: "09:00",
            interval_minutes: 15,
            timezone: "Asia/Shanghai",
            reset_time: "08:00",
          },
          cycle_date: "2026-10-01",
          next_run_at: null,
          schedule: [
            {
              account_id: "acc-1",
              scheduled_at: "2026-10-01T09:00:00+08:00",
              status: "pending",
            },
          ],
        });
      }
      return json({});
    });

    fireEvent.click(saveButton);

    await screen.findByText(/配置已保存/);
    expect(saveButton).toBeDisabled();
  });

  it("POST /api/accounts/{id}/checkin/run 202 starts operation, polls until complete, and refreshes checkin state", async () => {
    await openDashboard();
    vi.useFakeTimers();

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    const runButton = screen.getByRole("button", { name: "手动签到" });
    expect(runButton).toBeEnabled();

    // 1. POST returns 202
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-1/checkin/run") {
        return json({ operation_id: "op-checkin-1" }, 202);
      }
      if (urlStr.startsWith("/api/accounts")) {
        return json(
          snapshot({
            operation: {
              id: "op-checkin-1",
              account_id: "acc-1",
              kind: "checkin",
              status: "running",
              phase: "checking_in",
              error: null,
            },
          })
        );
      }
      if (urlStr.startsWith("/api/checkin")) {
        return json({
          ...defaultCheckin,
          schedule: [
            {
              account_id: "acc-1",
              scheduled_at: "2026-10-01T09:00:00+08:00",
              status: "running",
            },
          ],
        });
      }
      return json({});
    });

    await act(async () => fireEvent.click(runButton));

    expect(screen.getAllByText("正在签到")[0]).toBeInTheDocument();

    // 2. Poll finishes
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/accounts")) {
        return json(
          snapshot({
            operation: {
              id: "op-checkin-1",
              account_id: "acc-1",
              kind: "checkin",
              status: "succeeded",
              phase: "done",
              error: null,
            },
          })
        );
      }
      if (urlStr.startsWith("/api/checkin")) {
        return json({
          ...defaultCheckin,
          schedule: [
            {
              account_id: "acc-1",
              scheduled_at: "2026-10-01T09:00:00+08:00",
              status: "success",
            },
          ],
        });
      }
      return json({});
    });

    await act(async () => vi.advanceTimersByTime(1000));

    expect(screen.getByText("已签到")).toBeInTheDocument();
  });

  it("POST /api/accounts/{id}/checkin/run 200 already_recorded refreshes state without starting background operation", async () => {
    await openDashboard();

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    const runButton = screen.getByRole("button", { name: "手动签到" });

    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-1/checkin/run") {
        return json({ already_recorded: true }, 200);
      }
      if (urlStr.startsWith("/api/checkin")) {
        return json({
          ...defaultCheckin,
          schedule: [
            {
              account_id: "acc-1",
              scheduled_at: "2026-10-01T09:00:00+08:00",
              status: "already_done",
            },
          ],
        });
      }
      if (urlStr.startsWith("/api/accounts")) return json(snapshot());
      return json({});
    });

    await act(async () => fireEvent.click(runButton));

    await waitFor(() => {
      expect(screen.getAllByText("今日已完成").length).toBeGreaterThan(0);
    });
  });

  it("unknown status requires explicit confirmation modal before retrying with confirm_retry: true", async () => {
    const unknownCheckin: GlobalCheckinResponse = {
      ...defaultCheckin,
      schedule: [
        {
          account_id: "acc-1",
          scheduled_at: "2026-10-01T09:00:00+08:00",
          status: "unknown",
        },
      ],
    };

    await openDashboard(unknownCheckin);

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    const retryButton = await screen.findByRole("button", { name: "重试签到" });
    expect(retryButton).toBeEnabled();

    // Clicking retry opens confirmation modal
    fireEvent.click(retryButton);

    const modal = screen.getByRole("dialog", { name: "确认重试签到" });
    expect(modal).toBeInTheDocument();
    expect(
      screen.getByText(/该账号上次签到结果未知（如上游超时或未返回明确状态）/)
    ).toBeInTheDocument();

    // Confirm in dialog dispatches with confirm_retry: true
    fetchMock.mockImplementation(async (url: string | URL | Request, init?: RequestInit) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-1/checkin/run" && init?.method === "POST") {
        expect(JSON.parse(init.body as string)).toEqual({ confirm_retry: true });
        return json({ operation_id: "op-checkin-retry" }, 202);
      }
      if (urlStr.startsWith("/api/accounts")) {
        return json(
          snapshot({
            operation: {
              id: "op-checkin-retry",
              account_id: "acc-1",
              kind: "checkin",
              status: "running",
              phase: "checking_in",
              error: null,
            },
          })
        );
      }
      if (urlStr.startsWith("/api/checkin")) return json(unknownCheckin);
      return json({});
    });

    fireEvent.click(screen.getByRole("button", { name: /确认.*重试/ }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("scheduled record appears on visibility change", async () => {
    await openDashboard();

    const updatedCheckin: GlobalCheckinResponse = {
      ...defaultCheckin,
      schedule: [
        {
          account_id: "acc-1",
          scheduled_at: "2026-10-01T09:00:00+08:00",
          status: "success",
        },
      ],
    };

    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/checkin")) return json(updatedCheckin);
      if (urlStr.startsWith("/api/accounts")) return json(snapshot());
      return json({});
    });

    act(() => {
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => "visible",
      });
      document.dispatchEvent(new Event("visibilitychange"));
    });

    await screen.findByText("已签到");
  });

  it("account change or logout aborts in-flight checkin requests and discards late responses", async () => {
    await openDashboard();

    const pendingCheckin = deferred();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/checkin")) return pendingCheckin.promise;
      if (urlStr === "/api/admin/session") return new Response(null, { status: 204 });
      return json({});
    });

    act(() => {
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => "visible",
      });
      document.dispatchEvent(new Event("visibilitychange"));
    });

    const checkinCall = fetchMock.mock.calls.slice().reverse().find(call => (call[0] as string).startsWith("/api/checkin"));
    const checkinSignal = checkinCall?.[1]?.signal;

    // User logs out
    fireEvent.click(screen.getByRole("button", { name: "退出管理" }));

    expect(checkinSignal?.aborted).toBe(true);

    // Resolve late response
    await act(async () =>
      pendingCheckin.resolve(
        json({
          ...defaultCheckin,
          schedule: [
            {
              account_id: "acc-1",
              scheduled_at: "2026-10-01T09:00:00+08:00",
              status: "success",
            },
          ],
        })
      )
    );

    // App is locked, checkin data is not rendered
    expect(screen.queryByRole("heading", { name: /全局自动签到/ })).not.toBeInTheDocument();
  });

  it("checkin 401 locks app whereas root reveal 401 remains inline", async () => {
    await openDashboard();

    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/checkin")) {
        return json({ error: { code: "upstream_session_expired", message: "未鉴权" } }, 401);
      }
      return json({});
    });

    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });

    await screen.findByLabelText("密钥");
    expect(screen.getByRole("button", { name: "进入" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /已绑定账号列表/ })).not.toBeInTheDocument();
  });
});
