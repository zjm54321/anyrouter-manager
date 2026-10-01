import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type {
  AccountDTO,
  AccountsResponse,
  CandidateDTO,
  GlobalCheckinResponse,
  OperationDTO,
  RequestLogsResponse,
} from "./api";

const balance = {
  quota_raw: "1000000.25",
  used_quota_raw: "25000",
  fetched_at: "2026-09-30T12:00:00Z",
};

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

const accountB: AccountDTO = {
  id: "acc-2",
  revision: "rev-2",
  username: "secondary-user",
  upstream_user_id: "20",
  balance: {
    quota_raw: "500000",
    used_quota_raw: "10000",
    fetched_at: "2026-09-30T12:00:00Z",
  },
  keys: [{ id: "key-2", name: "Secondary key", masked: "sk-cd…5678", enabled: true }],
  selected_key: { id: "key-2", name: "Secondary key", masked: "sk-cd…5678" },
  added_at: "2026-09-30T12:10:00Z",
};

const candidate: CandidateDTO = {
  id: "candidate-1",
  username: "next-user",
  upstream_user_id: "30",
  balance,
  keys: [{ id: "key-3", name: "Candidate key", masked: "sk-ef…9999", enabled: true }],
  expires_at: "2099-09-30T12:15:00Z",
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
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const unauthorized = () =>
  json({ error: { code: "invalid_credentials", message: "鉴权失败" } }, 401);

const deferred = () => {
  let resolve!: (response: Response) => void;
  const promise = new Promise<Response>(done => {
    resolve = done;
  });
  return { promise, resolve };
};

let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

beforeEach(() => {
  window.history.pushState(null, "", "#accounts");
  window.location.hash = "#accounts";
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
  window.history.pushState(null, "", "#accounts");
  window.location.hash = "#accounts";
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function open(
  state: AccountsResponse = snapshot(),
  checkin: GlobalCheckinResponse = defaultCheckin,
  logs: RequestLogsResponse = emptyLogs
) {
  fetchMock.mockImplementation(async (url: string | URL | Request) => {
    const urlStr = typeof url === "string" ? url : url.toString();
    if (urlStr.startsWith("/api/accounts")) {
      return json(state);
    }
    if (urlStr.startsWith("/api/checkin")) {
      return json(checkin);
    }
    if (urlStr.startsWith("/api/request-logs")) {
      return json(logs);
    }
    return json({});
  });

  window.location.hash = "#accounts";
  render(<App />);
  await screen.findByRole("heading", { name: /已绑定账号列表/ });
}

async function openReveal() {
  await open();
  fireEvent.click(screen.getByTestId("account-card-acc-1"));
  const button = screen.getByRole("button", { name: "查看密钥" });
  button.focus();
  fireEvent.click(button);
  fireEvent.change(await screen.findByLabelText("密钥"), {
    target: { value: "local-root" },
  });
}

function dispatchReveal() {
  fireEvent.click(screen.getByRole("button", { name: "验证并显示" }));
}

describe("管理员会话与操作", () => {
  it("GET 401 locks locally, establishes cookie session without a body, clears root at dispatch", async () => {
    const initialLoad = deferred();
    const task = deferred();
    let sessionEstablished = false;
    fetchMock.mockImplementation(async (url: string | URL | Request, init?: RequestInit) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/admin/session" && init?.method === "POST") {
        const response = await task.promise;
        sessionEstablished = response.ok;
        return response;
      }
      if (urlStr === "/api/accounts") {
        return sessionEstablished ? json(snapshot()) : initialLoad.promise;
      }
      if (urlStr.startsWith("/api/checkin")) return json(defaultCheckin);
      if (urlStr.startsWith("/api/request-logs")) return json(emptyLogs);
      throw new Error("Unexpected management endpoint");
    });

    render(<App />);
    // Flush the loading -> locked transition and its credential-clearing effect
    // before typing; finding the newly rendered input alone is not this barrier.
    await act(async () => initialLoad.resolve(unauthorized()));
    const input = screen.getByLabelText("密钥");
    const submit = screen.getByRole("button", { name: "进入" });
    expect(input).toBeEnabled();
    expect(input).toHaveValue("");
    expect(submit).toBeDisabled();
    fireEvent.change(input, { target: { value: "local-root" } });
    await waitFor(() => {
      expect(input).toHaveValue("local-root");
      expect(submit).toBeEnabled();
    });

    fireEvent.click(submit);
    expect(input).toHaveValue("");
    const sessionCalls = fetchMock.mock.calls.filter(
      ([url, init]) => url === "/api/admin/session" && init?.method === "POST"
    );
    expect(sessionCalls).toHaveLength(1);
    const [sessionCall] = sessionCalls;
    expect(sessionCall).toEqual([
      "/api/admin/session",
      expect.objectContaining({
        method: "POST",
        headers: { Authorization: "Bearer local-root" },
        credentials: "same-origin",
        cache: "no-store",
      }),
    ]);
    expect(sessionCall[1]?.body).toBeUndefined();
    expect(screen.getByRole("button", { name: "正在建立会话…" })).toBeDisabled();
    expect(screen.queryByText("current-user")).not.toBeInTheDocument();
    await act(async () => task.resolve(new Response(null, { status: 204 })));
    await waitFor(() => {
      expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();
    });
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
  });

  it("clears password immediately, reads candidate after 202 and activates only selected existing key", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "添加账号" }));
    fireEvent.change(await screen.findByLabelText(/用户名/), {
      target: { value: "next-user" },
    });
    const password = screen.getByLabelText(/密码/);
    fireEvent.change(password, { target: { value: "once-only" } });
    const loginTask = deferred();

    // Custom fetch handler for login sequence
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/login") {
        return loginTask.promise;
      }
      if (urlStr.startsWith("/api/accounts")) {
        return json(
          snapshot({
            candidate,
            operation: {
              id: "login-op",
              kind: "login",
               status: "succeeded",
              phase: "reading_account",
              account_id: null,
              error: null,
            },
          })
        );
      }
      if (urlStr.startsWith("/api/checkin")) return json(defaultCheckin);
      if (urlStr.startsWith("/api/request-logs")) return json(emptyLogs);
      return json({});
    });

    fireEvent.click(screen.getByRole("button", { name: "登录" }));
    expect(password).toHaveValue("");

    const loginCall = fetchMock.mock.calls.find(call => call[0] === "/api/accounts/login");
    expect(loginCall).toBeDefined();
    expect(JSON.parse(loginCall![1]?.body as string)).toEqual({
      username: "next-user",
      password: "once-only",
    });
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();

    await act(async () => loginTask.resolve(json({ operation_id: "login-op" }, 202)));

    const saveButton = await screen.findByRole("button", { name: "保存账号" });
    expect(saveButton).toBeEnabled();
    expect(screen.getByText("Candidate key")).toBeInTheDocument();
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();

    // Select candidate key
    const radio = screen.getAllByRole("radio")[0];
    fireEvent.click(radio);

    // Save candidate
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/save") {
        return json({ operation_id: "save-op" }, 202);
      }
      if (urlStr.startsWith("/api/accounts")) {
        return json(
          snapshot({
            accounts: [
              accountA,
              {
                id: "acc-2",
                revision: "rev-2",
                username: "next-user",
                upstream_user_id: "30",
                balance,
                keys: [{ id: "key-3", name: "Candidate key", masked: "sk-ef…9999", enabled: true }],
                selected_key: { id: "key-3", name: "Candidate key", masked: "sk-ef…9999" },
                added_at: "2026-09-30T12:20:00Z",
              },
            ],
             candidate: null,
             operation: { id: "save-op", kind: "save", status: "succeeded", phase: "done", error: null, account_id: "acc-2" },
          })
        );
      }
      if (urlStr.startsWith("/api/checkin")) return json(defaultCheckin);
      if (urlStr.startsWith("/api/request-logs")) return json(emptyLogs);
      return json({});
    });

    fireEvent.click(saveButton);

    await waitFor(() => {
      const saveCall = fetchMock.mock.calls.find(call => call[0] === "/api/accounts/save");
      expect(saveCall).toBeDefined();
      expect(JSON.parse(saveCall![1]?.body as string)).toEqual({
        candidate_id: "candidate-1",
        key_id: "key-3",
      });
    });

    // Both current-user and next-user are present!
    expect(await screen.findByTestId("account-card-acc-2")).toHaveTextContent("next-user");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();
  });

  it("candidate with no existing keys can be saved (disabled for route), and shows login for expired candidates", async () => {
    await open(snapshot({ candidate: { ...candidate, keys: [] } }));
    expect(screen.getByText("未选择密钥，仅管理账号")).toBeInTheDocument();
    const saveButton = screen.getByRole("button", { name: "保存账号" });
    expect(saveButton).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "重新登录" }));
    expect(screen.getByLabelText(/密码/)).toBeInTheDocument();
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();
  });

  it("copies the backend development gateway only on click with 2s feedback", async () => {
    await open();
    await act(async () => {
      fireEvent.click(screen.getByRole("link", { name: /设置/ }));
    });
    const copyBtn = await screen.findByRole("button", { name: "复制地址" });
    vi.useFakeTimers();
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(copyBtn);
    });
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(`${window.location.origin}/v1`);
    expect(screen.getByText("已复制网关地址")).toBeInTheDocument();
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
    expect(screen.queryByText("已复制网关地址")).not.toBeInTheDocument();
    vi.useRealTimers();
  });

  it("expired candidate presents login instead of activation", async () => {
    await open(snapshot({ candidate: { ...candidate, expires_at: "2000-01-01T00:00:00Z" } }));
    expect(screen.getByText("候选账号已过期，请重新登录")).toBeInTheDocument();
    expect(screen.getByLabelText(/密码/)).toBeInTheDocument();
  });

  it("polls only running operations at 1s, preserves old snapshot during refresh and upstream failure", async () => {
    await open();
    vi.useFakeTimers();
    const running: OperationDTO = {
      id: "refresh-op",
      account_id: "acc-1",
      kind: "refresh",
      status: "running",
      phase: "reading_account",
      error: null,
    };

    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-1/refresh") {
        return json({ operation_id: running.id }, 202);
      }
      if (urlStr.startsWith("/api/accounts")) {
        return json(snapshot({ operation: running }));
      }
      if (urlStr.startsWith("/api/checkin")) return json(defaultCheckin);
      if (urlStr.startsWith("/api/request-logs")) return json(emptyLogs);
      return json({});
    });

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "刷新余额" })));
    expect(screen.getByText("$1.95")).toBeInTheDocument();
    expect(screen.getByText("正在读取余额")).toBeInTheDocument();

    // Next poll returns failed
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/accounts")) {
        return json(
          snapshot({
            operation: {
              ...running,
              status: "failed",
              error: { code: "upstream_challenge", message: "safe" },
            },
          })
        );
      }
      if (urlStr.startsWith("/api/checkin")) return json(defaultCheckin);
      if (urlStr.startsWith("/api/request-logs")) return json(emptyLogs);
      return json({});
    });

    await act(async () => vi.advanceTimersByTime(1000));
    expect(screen.getAllByText("上游仍要求验证，暂时无法完成登录").length).toBeGreaterThan(0);
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();
    expect(screen.getByText("$1.95")).toBeInTheDocument();
  });

  it("stops polling on transport error and offers retry without clearing old data", async () => {
    await open();
    vi.useFakeTimers();

    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-1/refresh") {
        return json({ operation_id: "op" }, 202);
      }
      if (urlStr.startsWith("/api/accounts")) {
        return json(
          snapshot({
            operation: {
              id: "op",
              account_id: "acc-1",
              kind: "refresh",
              status: "running",
              phase: "reading_account",
              error: null,
            },
          })
        );
      }
      return json({});
    });

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "刷新余额" })));

    // Transport error
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/accounts")) {
        throw new TypeError("network");
      }
      return json({});
    });

    await act(async () => vi.advanceTimersByTime(1000));
    expect(screen.getAllByRole("button", { name: "重试读取状态" })[0]).toBeInTheDocument();
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();

    // Retry recovery
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/accounts")) return json(snapshot());
      return json({});
    });

    await act(async () =>
      fireEvent.click(screen.getAllByRole("button", { name: "重试读取状态" })[0])
    );
    expect(screen.queryByRole("button", { name: "重试读取状态" })).not.toBeInTheDocument();
  });

  it("POST 401 remains inline; GET account 401 clears session state", async () => {
    await open();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-1/refresh") return unauthorized();
      if (urlStr.startsWith("/api/accounts")) return json(snapshot());
      return json({});
    });

    fireEvent.click(screen.getByRole("button", { name: "刷新余额" }));
    await screen.findByText(/管理请求未通过鉴权/);
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();

    // GET accounts returns 401
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.startsWith("/api/accounts")) return unauthorized();
      return json({});
    });

    fireEvent.click(screen.getByRole("button", { name: "退出管理" }));
    await screen.findByLabelText("密钥");
    expect(screen.queryByText("current-user")).not.toBeInTheDocument();
  });

  it("logout aborts pending requests, resets all local data even if revocation fails, ignores old response", async () => {
    await open();
    const pending = deferred();
    fetchMock.mockImplementation(async (url: string | URL | Request, init?: RequestInit) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-1/refresh") return pending.promise;
      if (urlStr === "/api/admin/session" && init?.method === "DELETE") {
        throw new TypeError("network");
      }
      return json({});
    });

    fireEvent.click(screen.getByRole("button", { name: "刷新余额" }));
    const refreshCall = fetchMock.mock.calls.find(call => call[0] === "/api/accounts/acc-1/refresh");
    const signal = refreshCall?.[1]?.signal;

    fireEvent.click(screen.getByRole("button", { name: "退出管理" }));
    expect(signal?.aborted).toBe(true);
    await screen.findByRole("button", { name: "重试注销" });
    expect(screen.queryByText("current-user")).not.toBeInTheDocument();
    expect(screen.getByText(/服务端会话注销失败/)).toBeInTheDocument();

    await act(async () => pending.resolve(json({ operation_id: "late" }, 202)));
    expect(screen.queryByRole("heading", { name: /已绑定账号列表/ })).not.toBeInTheDocument();
  });
});

describe("密钥查看的短时授权", () => {
  it("sends revision and root, clears root at dispatch; 401 does not log out", async () => {
    await openReveal();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.includes("/key/reveal")) return unauthorized();
      return json({});
    });

    dispatchReveal();
    expect(screen.getByLabelText("密钥")).toHaveValue("");

    const revealCall = fetchMock.mock.calls.find(call => String(call[0]).includes("/key/reveal"));
    expect(revealCall).toBeDefined();
    expect(revealCall![0]).toBe("/api/accounts/acc-1/key/reveal");
    expect(revealCall![1]).toEqual(
      expect.objectContaining({
        method: "POST",
        headers: {
          Authorization: "Bearer local-root",
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ revision: "rev-1" }),
      })
    );

    await screen.findByText(/本地 root 密钥不正确|未通过验证/);
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();
  });

  it("close aborts request and discards late response; Escape returns focus", async () => {
    await openReveal();
    const task = deferred();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.includes("/key/reveal")) return task.promise;
      return json({});
    });

    dispatchReveal();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });

    const revealCall = fetchMock.mock.calls.find(call => String(call[0]).includes("/key/reveal"));
    expect(revealCall![1]?.signal?.aborted).toBe(true);

    await act(async () =>
      task.resolve(
        json({
          account_id: "acc-1",
          revision: "rev-1",
          key_id: "key-1",
          key: "never-show",
        })
      )
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("drops mismatching revision payload", async () => {
    await openReveal();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.includes("/key/reveal")) {
        return json({
          account_id: "acc-1",
          revision: "rev-2", // mismatch!
          key_id: "key-1",
          key: "never-show",
        });
      }
      return json({});
    });

    dispatchReveal();
    await screen.findByText(/已丢弃返回的密钥|账号版本已变化/);
    expect(screen.queryByDisplayValue("never-show")).not.toBeInTheDocument();
  });

  it("stale revision rejection stays inline without revealing or locking management", async () => {
    await openReveal();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.includes("/key/reveal")) {
        return json({ error: { code: "stale_revision", message: "stale" } }, 409);
      }
      return json({});
    });

    dispatchReveal();
    await screen.findByText(/账号已被其他操作修改|账号版本已变化/);
    expect(screen.queryByLabelText(/当前密钥/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();
  });

  it("explicit close clears the displayed value and reopening requires fresh authorization", async () => {
    await openReveal();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.includes("/key/reveal")) {
        return json({
          account_id: "acc-1",
          revision: "rev-1",
          key_id: "key-1",
          key: "short-lived",
        });
      }
      return json({});
    });

    dispatchReveal();
    await screen.findByLabelText(/当前密钥/);
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.queryByDisplayValue("short-lived")).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "查看密钥" }));
    expect(screen.getByLabelText("密钥")).toHaveValue("");
    expect(screen.queryByDisplayValue("short-lived")).not.toBeInTheDocument();
  });

  it("auto hides at 30s, copies only on explicit click, traps Tab", async () => {
    await openReveal();
    fireEvent.change(screen.getByLabelText("密钥"), { target: { value: "local-root" } });
    screen.getByRole("button", { name: "关闭对话框" }).focus();
    fireEvent.keyDown(screen.getByRole("button", { name: "关闭对话框" }), {
      key: "Tab",
      shiftKey: true,
    });
    expect(screen.getByRole("button", { name: "验证并显示" })).toHaveFocus();

    vi.useFakeTimers();
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr.includes("/key/reveal")) {
        return json({
          account_id: "acc-1",
          revision: "rev-1",
          key_id: "key-1",
          key: "short-lived",
        });
      }
      return json({});
    });

    await act(async () => dispatchReveal());
    expect(screen.getByLabelText(/当前密钥/)).toHaveValue("short-lived");
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();

    await act(async () => fireEvent.click(screen.getByRole("button", { name: "复制" })));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("short-lived");

    await act(async () => vi.advanceTimersByTime(29_999));
    expect(screen.getByLabelText(/当前密钥/)).toHaveValue("short-lived");
    await act(async () => vi.advanceTimersByTime(1));
    expect(screen.queryByLabelText(/当前密钥/)).not.toBeInTheDocument();
    expect(screen.getByText("已自动隐藏密钥。如需查看，请重新验证")).toBeInTheDocument();
  });
});

describe("多账号与路由切换", () => {
  it("displays multiple accounts A and B with raw balances, masks, and checkin status", async () => {
    await open(
      snapshot({
        accounts: [accountA, accountB],
        route_account_id: "acc-1",
      }),
      {
        ...defaultCheckin,
        schedule: [
          {
            account_id: "acc-1",
            scheduled_at: "2026-10-01T09:00:00+08:00",
            status: "success",
          },
          {
            account_id: "acc-2",
            scheduled_at: "2026-10-01T09:30:00+08:00",
            status: "pending",
          },
        ],
      }
    );

    expect(screen.getAllByText("current-user")[0]).toBeInTheDocument();
    expect(screen.getByText("secondary-user")).toBeInTheDocument();
    expect(screen.getByText("$1.95")).toBeInTheDocument();
    expect(screen.getByText("$0.05")).toBeInTheDocument();
    expect(screen.getByText("$0.98")).toBeInTheDocument();
    expect(screen.getByText("$0.02")).toBeInTheDocument();
    expect(await screen.findByText("已签到")).toBeInTheDocument();
  });

  it("viewing details does NOT route and fetches account checkin history", async () => {
    await open(
      snapshot({
        accounts: [accountA, accountB],
        route_account_id: "acc-1",
      })
    );

    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-2/checkin") {
        return json({
          account_id: "acc-2",
          cycle_date: "2026-10-01",
          today: null,
          history: [
            {
              date: "2026-09-30",
              account_user_id: "20",
              trigger: "scheduled",
              status: "success",
              started_at: "2026-09-30T09:30:00Z",
              finished_at: "2026-09-30T09:30:02Z",
              code: "ok",
            },
          ],
          next_run_at: null,
        });
      }
      return json({});
    });

    const viewDetailButtons = screen.getAllByRole("button", { name: "查看详情" });
    fireEvent.click(viewDetailButtons[1]); // View details of account B

    await screen.findByText(/账号签到详情 · secondary-user/);
    // Route call was NOT dispatched!
    const routeCalls = fetchMock.mock.calls.filter(call => String(call[0]).includes("/route"));
    expect(routeCalls.length).toBe(0);
    // Account A is still routed!
    expect(within(screen.getByTestId("account-card-acc-1")).getByText("当前路由")).toBeInTheDocument();
  });

  it("route button confirms and updates only on server 200, failure keeps original route", async () => {
    await open(
      snapshot({
        accounts: [accountA, accountB],
        route_account_id: "acc-1",
      })
    );

    // Click account B to open detail drawer
    fireEvent.click(screen.getByTestId("account-card-acc-2"));

    const routeButton = screen.getByRole("button", { name: "使用此账号" });
    expect(routeButton).toBeEnabled();

    // 1. Simulate failure on route
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-2/route") {
        return json({ error: { code: "server_error", message: "路由切换失败" } }, 500);
      }
      return json({});
    });

    fireEvent.click(routeButton);
    await screen.findByText("路由切换失败");
    // Original routed account is still acc-1!
    expect(within(screen.getByTestId("account-card-acc-1")).getByText("当前路由")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "使用此账号" })).toBeInTheDocument();

    // 2. Simulate success on route
    fetchMock.mockImplementation(async (url: string | URL | Request) => {
      const urlStr = typeof url === "string" ? url : url.toString();
      if (urlStr === "/api/accounts/acc-2/route") {
        return json(
          snapshot({
            accounts: [accountA, accountB],
            route_account_id: "acc-2",
          }),
          200
        );
      }
      return json({});
    });

    fireEvent.click(screen.getByRole("button", { name: "使用此账号" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "当前正在使用" })).toBeInTheDocument();
    });
  });

  it("account with no key has route button disabled", async () => {
    const accountNoKey: AccountDTO = {
      ...accountB,
      id: "acc-3",
      keys: [],
      selected_key: null,
    };

    await open(
      snapshot({
        accounts: [accountA, accountNoKey],
        route_account_id: "acc-1",
      })
    );

    fireEvent.click(screen.getByTestId("account-card-acc-3"));
    expect(screen.getByRole("button", { name: "未选择密钥" })).toBeDisabled();
  });

  it("navigates across 4 pages via links, updates aria-current, handles browser back/forward and fallback", async () => {
    await open();

    // 1. Overview page
    const overviewLink = screen.getByRole("link", { name: /首页概览/ });
    fireEvent.click(overviewLink);
    expect(overviewLink).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "首页概览" })).toBeInTheDocument();
    expect(window.location.hash).toBe("#overview");

    // 2. Accounts page
    const accountsLink = screen.getByRole("link", { name: /账号列表/ });
    fireEvent.click(accountsLink);
    expect(accountsLink).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "已绑定账号列表" })).toBeInTheDocument();
    expect(window.location.hash).toBe("#accounts");

    // 3. Logs page
    const logsLink = screen.getByRole("link", { name: /网关请求日志/ });
    fireEvent.click(logsLink);
    expect(logsLink).toHaveAttribute("aria-current", "page");
    expect(screen.getAllByRole("heading", { name: "网关请求日志" }).length).toBeGreaterThan(0);
    expect(window.location.hash).toBe("#logs");

    // 4. Settings page
    const settingsLink = screen.getByRole("link", { name: /设置/ });
    fireEvent.click(settingsLink);
    expect(settingsLink).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "系统设置" })).toBeInTheDocument();
    expect(window.location.hash).toBe("#settings");

    // 5. Browser back simulation via hashchange
    await act(async () => {
      window.location.hash = "#accounts";
      window.dispatchEvent(new HashChangeEvent("hashchange"));
    });
    expect(accountsLink).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "已绑定账号列表" })).toBeInTheDocument();

    // 6. Unknown hash falls back to overview
    await act(async () => {
      window.history.pushState(null, "", "#unknown-page");
      window.location.hash = "#unknown-page";
      window.dispatchEvent(new HashChangeEvent("hashchange"));
    });
    expect(overviewLink).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "首页概览" })).toBeInTheDocument();

    // Reset hash to accounts for subsequent tests
    await act(async () => {
      window.location.hash = "#accounts";
      window.dispatchEvent(new HashChangeEvent("hashchange"));
    });
  });

  it("does not display fixed upstream banner or URL in the UI", async () => {
    await open();
    expect(screen.queryByText(/固定上游/)).not.toBeInTheDocument();
    expect(screen.queryByText(/https:\/\/anyrouter\.top/i)).not.toBeInTheDocument();
  });

  it("renders raw error body in pre tag without HTML interpretation", async () => {
    const errorLogs: RequestLogsResponse = {
      items: [
        {
          id: "log-err-1",
          timestamp: "2026-10-01T12:00:00Z",
          account_id: "acc-1",
          account_name: "test-user",
          http_status: 500,
          error_body: '<div id="injection">Raw Backend Error Details</div>',
          truncated: false,
        },
      ],
      dropped_count: 0,
    };

    await open(snapshot(), defaultCheckin, errorLogs);

    // Navigate to logs page
    fireEvent.click(screen.getByRole("link", { name: /网关请求日志/ }));

    // Click to expand error details
    const expandBtn = await screen.findByRole("button", { name: "查看错误原文" });
    fireEvent.click(expandBtn);

    // Verify error text is inside pre
    const preEl = screen.getByText(/Raw Backend Error Details/);
    expect(preEl.tagName.toLowerCase()).toBe("pre");
    // Verify it was NOT rendered as an HTML DOM element with id="injection"
    expect(document.getElementById("injection")).toBeNull();
  });

  it("cleans up active account detail view when navigating between pages", async () => {
    await open();

    // On Accounts page, view details of acc-1
    const viewDetailBtn = screen.getByRole("button", { name: "查看详情" });
    fireEvent.click(viewDetailBtn);
    expect(await screen.findByRole("heading", { name: /账号签到详情/ })).toBeInTheDocument();

    // Navigate away to Settings
    fireEvent.click(screen.getByRole("link", { name: /设置/ }));
    expect(screen.queryByRole("heading", { name: /账号签到详情/ })).not.toBeInTheDocument();

    // Navigate back to Accounts: detail view was reset
    fireEvent.click(screen.getByRole("link", { name: /账号列表/ }));
    expect(screen.queryByRole("heading", { name: /账号签到详情/ })).not.toBeInTheDocument();
  });
});
