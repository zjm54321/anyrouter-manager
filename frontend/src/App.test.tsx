import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { AccountState, ActiveAccount, CandidateAccount } from "./api";

const balance = { quota_raw: "1000000.25", used_quota_raw: "25000", fetched_at: "2026-09-30T12:00:00Z" };
const active: ActiveAccount = {
  revision: "rev-1", username: "current-user", upstream_user_id: "10", balance,
  selected_key: { id: "key-1", name: "Existing key", masked: "sk-ab…1234" },
  activated_at: "2026-09-30T12:00:00Z",
};
const candidate: CandidateAccount = {
  id: "candidate-1", username: "next-user", upstream_user_id: "20", balance,
  keys: [{ id: "key-2", name: "Candidate key", masked: "sk-cd…5678" }],
  expires_at: "2099-09-30T12:15:00Z",
};
const snapshot = (extra: Partial<AccountState> = {}): AccountState => ({ active, candidate: null, operation: null, ...extra });
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const unauthorized = () => json({ error: { code: "invalid_credentials", message: "鉴权失败" } }, 401);
const deferred = () => {
  let resolve!: (response: Response) => void;
  const promise = new Promise<Response>(done => { resolve = done; });
  return { promise, resolve };
};
const defaultCheckin = {
  settings: { enabled: false, time: "09:00", timezone: "Asia/Shanghai" as const, reset_time: "08:00" as const },
  cycle_date: "2026-10-01",
  today: null,
  history: [],
  next_run_at: null,
};

let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

beforeEach(() => {
  fetchMock = vi.fn<typeof fetch>();
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("navigator", Object.assign(Object.create(navigator), { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } }));
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

async function open(state = snapshot(), checkin = defaultCheckin) {
  fetchMock.mockResolvedValueOnce(json(state));
  fetchMock.mockResolvedValueOnce(json(checkin));
  render(<App />);
  await screen.findByRole("heading", { name: "当前账号" });
  await screen.findByRole("heading", { name: "每日自动签到" });
}
async function openReveal() {
  await open();
  const button = screen.getByRole("button", { name: "查看密钥" });
  button.focus();
  fireEvent.click(button);
  fireEvent.change(await screen.findByLabelText("本地 root 密钥"), { target: { value: "local-root" } });
}
function dispatchReveal() {
  fireEvent.click(screen.getByRole("button", { name: "验证并显示" }));
}

describe("管理员会话与操作", () => {
  it("GET 401 locks locally, establishes cookie session without a body, clears root at dispatch", async () => {
    fetchMock.mockResolvedValueOnce(unauthorized());
    render(<App />);
    const input = await screen.findByLabelText("本地 root 密钥");
    fireEvent.change(input, { target: { value: "local-root" } });
    const task = deferred();
    fetchMock.mockReturnValueOnce(task.promise);
    fetchMock.mockResolvedValueOnce(json(snapshot()));
    fireEvent.click(screen.getByRole("button", { name: "进入管理页面" }));
    expect(input).toHaveValue("");
    expect(fetchMock.mock.calls[1]).toEqual(["/api/admin/session", expect.objectContaining({
      method: "POST", headers: { Authorization: "Bearer local-root" }, credentials: "same-origin", cache: "no-store",
    })]);
    expect(fetchMock.mock.calls[1][1]?.body).toBeUndefined();
    await act(async () => task.resolve(new Response(null, { status: 204 })));
    await screen.findByText("current-user");
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
  });

  it("clears password immediately, reads candidate after 202 and activates only selected existing key", async () => {
    await open();
    fireEvent.change(screen.getByLabelText("AnyRouter 用户名"), { target: { value: "next-user" } });
    const password = screen.getByLabelText("AnyRouter 密码");
    fireEvent.change(password, { target: { value: "once-only" } });
    const loginTask = deferred();
    fetchMock.mockReturnValueOnce(loginTask.promise);
    fetchMock.mockResolvedValueOnce(json(snapshot({ candidate })));
    fireEvent.click(screen.getByRole("button", { name: "登录并读取已有密钥" }));
    expect(password).toHaveValue("");
    expect(JSON.parse(fetchMock.mock.calls[2][1]?.body as string)).toEqual({ username: "next-user", password: "once-only" });
    expect(screen.getByText("current-user")).toBeInTheDocument();
    await act(async () => loginTask.resolve(json({ operation_id: "login-op" }, 202)));
    const activateButton = await screen.findByRole("button", { name: "确认启用所选密钥" });
    expect(activateButton).toBeDisabled();
    expect(screen.getByText("启用后将替换当前账号")).toBeInTheDocument();
    expect(screen.getByText("current-user")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio"));
    expect(activateButton).toBeEnabled();
    fetchMock.mockResolvedValueOnce(json({ operation_id: "activate-op" }, 202));
    fetchMock.mockResolvedValueOnce(json(snapshot({ active: { ...active, username: "next-user", revision: "rev-2" } })));
    fetchMock.mockResolvedValueOnce(json(defaultCheckin));
    fireEvent.click(activateButton);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(7));
    expect(JSON.parse(fetchMock.mock.calls[4][1]?.body as string)).toEqual({ candidate_id: "candidate-1", key_id: "key-2" });
    await waitFor(() => expect(screen.queryByText("current-user")).not.toBeInTheDocument());
  });

  it("disables activation when no existing keys, and shows login for expired candidates", async () => {
    await open(snapshot({ candidate: { ...candidate, keys: [] } }));
    expect(screen.getByText("未找到已有密钥，请先在 AnyRouter 创建后重试")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认启用所选密钥" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "重新登录并读取" }));
    expect(screen.getByLabelText("AnyRouter 密码")).toBeInTheDocument();
    expect(screen.getByText("current-user")).toBeInTheDocument();
  });

  it("copies the backend development gateway only on click with 2s feedback", async () => {
    await open();
    vi.useFakeTimers();
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "复制地址" })));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("http://127.0.0.1:8080/v1");
    expect(screen.getByText("已复制网关地址")).toBeInTheDocument();
    await act(async () => vi.advanceTimersByTime(2000));
    expect(screen.queryByText("已复制网关地址")).not.toBeInTheDocument();
  });

  it("expired candidate presents login instead of activation", async () => {
    await open(snapshot({ candidate: { ...candidate, expires_at: "2000-01-01T00:00:00Z" } }));
    expect(screen.getByText("候选账号已过期，请重新登录")).toBeInTheDocument();
    expect(screen.getByLabelText("AnyRouter 密码")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "确认启用所选密钥" })).not.toBeInTheDocument();
  });

  it("polls only running operations at 1s, preserves old snapshot during refresh and upstream failure", async () => {
    await open();
    vi.useFakeTimers();
    const running = { id: "refresh-op", kind: "refresh", status: "running", phase: "reading_account", error: null } as const;
    fetchMock.mockResolvedValueOnce(json({ operation_id: running.id }, 202));
    fetchMock.mockResolvedValueOnce(json(snapshot({ operation: running })));
    fetchMock.mockResolvedValueOnce(json(snapshot({ operation: { ...running, status: "failed", error: { code: "upstream_challenge", message: "safe" } } })));
    fetchMock.mockResolvedValueOnce(json(defaultCheckin));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "刷新余额" })));
    expect(screen.getByText("1000000.25")).toBeInTheDocument();
    expect(screen.getByText("正在读取余额")).toBeInTheDocument();
    await act(async () => vi.advanceTimersByTime(999));
    expect(fetchMock).toHaveBeenCalledTimes(4);
    await act(async () => vi.advanceTimersByTime(1));
    expect(screen.getByText("上游仍要求验证，暂时无法完成登录")).toBeInTheDocument();
    expect(screen.getByText("current-user")).toBeInTheDocument();
    expect(screen.getByText("1000000.25")).toBeInTheDocument();
    await act(async () => vi.advanceTimersByTime(5000));
    expect(fetchMock).toHaveBeenCalledTimes(6);
  });

  it("stops polling on transport error and offers retry without clearing old data", async () => {
    await open();
    vi.useFakeTimers();
    fetchMock.mockResolvedValueOnce(json({ operation_id: "op" }, 202));
    fetchMock.mockResolvedValueOnce(json(snapshot({ operation: { id: "op", kind: "refresh", status: "running", phase: "reading_account", error: null } })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "刷新余额" })));
    fetchMock.mockRejectedValueOnce(new TypeError("network"));
    await act(async () => vi.advanceTimersByTime(1000));
    expect(screen.getByRole("button", { name: "重试读取状态" })).toBeInTheDocument();
    expect(screen.getByText("current-user")).toBeInTheDocument();
    await act(async () => vi.advanceTimersByTime(5000));
    expect(fetchMock).toHaveBeenCalledTimes(5);
    fetchMock.mockResolvedValueOnce(json(snapshot()));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "重试读取状态" })));
    expect(screen.queryByRole("button", { name: "重试读取状态" })).not.toBeInTheDocument();
  });

  it("POST 401 remains inline; GET account 401 clears session state", async () => {
    await open();
    fetchMock.mockResolvedValueOnce(unauthorized());
    fireEvent.click(screen.getByRole("button", { name: "刷新余额", hidden: true }));
    await screen.findByText("管理请求未通过鉴权，请重试或退出后重新建立管理会话");
    expect(screen.getByText("current-user")).toBeInTheDocument();
    fetchMock.mockResolvedValueOnce(json({ operation_id: "op" }, 202));
    fetchMock.mockResolvedValueOnce(unauthorized());
    fireEvent.click(screen.getByRole("button", { name: "刷新余额" }));
    await screen.findByLabelText("本地 root 密钥");
    expect(screen.queryByText("current-user")).not.toBeInTheDocument();
  });

  it("logout aborts pending requests, resets all local data even if revocation fails, ignores old response", async () => {
    await open();
    const pending = deferred();
    fetchMock.mockReturnValueOnce(pending.promise);
    fireEvent.click(screen.getByRole("button", { name: "刷新余额" }));
    const signal = fetchMock.mock.calls[2][1]?.signal;
    fetchMock.mockRejectedValueOnce(new TypeError("network"));
    fireEvent.click(screen.getByRole("button", { name: "退出管理" }));
    expect(signal?.aborted).toBe(true);
    await screen.findByRole("button", { name: "重试注销" });
    expect(screen.queryByText("current-user")).not.toBeInTheDocument();
    expect(screen.getByText(/服务端会话注销失败/)).toBeInTheDocument();
    await act(async () => pending.resolve(json({ operation_id: "late" }, 202)));
    expect(screen.queryByRole("heading", { name: "当前账号" })).not.toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledTimes(4);
  });
});

describe("密钥查看的短时授权", () => {
  it("sends revision and root, clears root at dispatch; 401 does not log out", async () => {
    await openReveal();
    fetchMock.mockResolvedValueOnce(unauthorized());
    dispatchReveal();
    expect(screen.getByLabelText("本地 root 密钥")).toHaveValue("");
    expect(fetchMock.mock.calls[2]).toEqual(["/api/account/key/reveal", expect.objectContaining({
      method: "POST", headers: { Authorization: "Bearer local-root", "Content-Type": "application/json" },
      body: JSON.stringify({ active_revision: "rev-1" }),
    })]);
    await screen.findByText("本地 root 密钥或管理会话未通过验证，请重试");
    expect(screen.getByText("current-user")).toBeInTheDocument();
  });

  it("close aborts request and discards late response; Escape returns focus", async () => {
    await openReveal();
    const task = deferred();
    fetchMock.mockReturnValueOnce(task.promise);
    dispatchReveal();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(fetchMock.mock.calls[2][1]?.signal?.aborted).toBe(true);
    expect(screen.getByRole("button", { name: "查看密钥" })).toHaveFocus();
    await act(async () => task.resolve(json({ active_revision: "rev-1", key_id: "key-1", key: "never-show" })));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "查看密钥" }));
    expect(screen.queryByDisplayValue("never-show")).not.toBeInTheDocument();
  });

  it("drops mismatching revision payload", async () => {
    await openReveal();
    fetchMock.mockResolvedValueOnce(json({ active_revision: "rev-2", key_id: "key-1", key: "never-show" }));
    dispatchReveal();
    await screen.findByText(/已丢弃返回的密钥/);
    expect(screen.queryByDisplayValue("never-show")).not.toBeInTheDocument();
  });

  it("stale revision rejection stays inline without revealing or locking management", async () => {
    await openReveal();
    fetchMock.mockResolvedValueOnce(json({ error: { code: "stale_revision", message: "stale" } }, 409));
    dispatchReveal();
    await screen.findByText("账号版本已变化，请关闭窗口后重新读取账号状态");
    expect(screen.queryByLabelText("当前密钥")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.getByText("current-user")).toBeInTheDocument();
  });

  it("explicit hide clears the displayed value and closing/reopening requires fresh authorization", async () => {
    await openReveal();
    fetchMock.mockResolvedValueOnce(json({ active_revision: "rev-1", key_id: "key-1", key: "short-lived" }));
    dispatchReveal();
    await screen.findByLabelText("当前密钥");
    fireEvent.click(screen.getByRole("button", { name: "隐藏 / 清除" }));
    expect(screen.queryByDisplayValue("short-lived")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    fireEvent.click(screen.getByRole("button", { name: "查看密钥" }));
    expect(screen.getByLabelText("本地 root 密钥")).toHaveValue("");
    expect(screen.queryByDisplayValue("short-lived")).not.toBeInTheDocument();
  });

  it("changed current revision aborts pending reveal and removes dialog", async () => {
    await openReveal();
    const task = deferred();
    fetchMock.mockReturnValueOnce(task.promise);
    dispatchReveal();
    fetchMock.mockResolvedValueOnce(json({ operation_id: "op-2" }, 202));
    fetchMock.mockResolvedValueOnce(json(snapshot({ active: { ...active, revision: "rev-2" } })));
    fetchMock.mockResolvedValueOnce(json(defaultCheckin));
    // Synthetic event intentionally exercises a state change from outside the dialog.
    fireEvent.click(screen.getByRole("button", { name: "刷新余额", hidden: true }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(fetchMock.mock.calls[2][1]?.signal?.aborted).toBe(true);
    await act(async () => task.resolve(json({ active_revision: "rev-1", key_id: "key-1", key: "never-show" })));
    expect(screen.queryByDisplayValue("never-show")).not.toBeInTheDocument();
  });

  it("logout cancels pending reveal and cannot restore it from a late response", async () => {
    await openReveal();
    const task = deferred();
    fetchMock.mockReturnValueOnce(task.promise);
    dispatchReveal();
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    fireEvent.click(screen.getByRole("button", { name: "退出管理", hidden: true }));
    await screen.findByText("已退出管理会话");
    expect(fetchMock.mock.calls[2][1]?.signal?.aborted).toBe(true);
    await act(async () => task.resolve(json({ active_revision: "rev-1", key_id: "key-1", key: "never-show" })));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue("never-show")).not.toBeInTheDocument();
  });

  it("auto hides at 30s, copies only on explicit click, traps Tab", async () => {
    await openReveal();
    screen.getByRole("button", { name: "关闭" }).focus();
    fireEvent.keyDown(screen.getByRole("button", { name: "关闭" }), { key: "Tab", shiftKey: true });
    expect(screen.getByRole("button", { name: "验证并显示" })).toHaveFocus();
    vi.useFakeTimers();
    fetchMock.mockResolvedValueOnce(json({ active_revision: "rev-1", key_id: "key-1", key: "short-lived" }));
    await act(async () => dispatchReveal());
    expect(screen.getByLabelText("当前密钥")).toHaveValue("short-lived");
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "复制密钥" })));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("short-lived");
    await act(async () => vi.advanceTimersByTime(29_999));
    expect(screen.getByLabelText("当前密钥")).toHaveValue("short-lived");
    await act(async () => vi.advanceTimersByTime(1));
    expect(screen.queryByLabelText("当前密钥")).not.toBeInTheDocument();
    expect(screen.getByText("已自动隐藏密钥。如需查看，请重新验证")).toBeInTheDocument();
  });
});
