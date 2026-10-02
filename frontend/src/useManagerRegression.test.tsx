import { StrictMode } from "react";
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountDTO, AccountsResponse, OperationDTO, SystemLogsResponse } from "./api";
import { useManager } from "./useManager";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const account = (id: string): AccountDTO => ({
  id, username: "local-fixture", upstream_user_id: "7", revision: "1",
  balance: { quota_raw: "500000", used_quota_raw: "0", fetched_at: "2026-10-01T12:00:00Z" },
  keys: [], selected_key: null, added_at: "2026-10-01T12:00:00Z",
});
const base = (): AccountsResponse => ({ accounts: [account("a"), account("b")], route_account_id: null, candidate: null, operation: null });
const operation = (id: string, kind: OperationDTO["kind"], status: OperationDTO["status"], account_id: string | null = null): OperationDTO => ({
  id, kind, status, account_id, phase: status === "running" ? "authenticating" : "done", error: null,
});
const json = (value: unknown, status = 200) => new Response(status === 204 ? null : JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } });
const settings = { level: "info", system_retention_days: 7, request_retention_days: 7 };
const checkin = (id: string) => ({ account_id: id, cycle_date: "2026-10-01", today: null, history: [], next_run_at: null });
const systemPage = (id: string): SystemLogsResponse => ({
  items: [{ id, timestamp: "2026-10-01T12:00:00Z", level: "info", event: "login_finish",
    operation_id: "login-1", account_id: null, stage: "helper_result", elapsed_ms: 28000,
    http_status: null, reason: "deadline_expired", diagnostics: null }],
  dropped_count: 0,
});
let snapshot: AccountsResponse;
let readAccounts: () => Promise<Response>;
let custom: (path: string, init: RequestInit) => Promise<Response> | undefined;
let reads: number;
let concurrent: number;
let maximum: number;
let posts: string[];
let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-01T12:00:00Z"));
  snapshot = base();
  reads = concurrent = maximum = 0;
  posts = [];
  custom = () => undefined;
  readAccounts = async () => json(snapshot);
  fetchMock = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const path = String(input);
    const method = init.method ?? "GET";
    if (method === "POST") posts.push(path);
    if (path === "/api/accounts" && method === "GET") {
      reads++; concurrent++; maximum = Math.max(maximum, concurrent);
      try { return await readAccounts(); } finally { concurrent--; }
    }
    const override = custom(path, init);
    if (override) return override;
    if (path === "/api/admin/session") return json(null, 204);
    if (path === "/api/accounts/login") return json({ operation_id: "login-1" }, 202);
    if (path === "/api/accounts/save") return json({ operation_id: "save-1" }, 202);
    if (path.endsWith("/refresh")) return json({ operation_id: "refresh-1" }, 202);
    if (path.endsWith("/key/select")) return json({ operation_id: "key-1" }, 202);
    if (path.endsWith("/checkin/run")) return json({ operation_id: "checkin-1" }, 202);
    if (path === "/api/checkin") return json({ settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" }, cycle_date: "2026-10-01", next_run_at: null, schedule: [] });
    if (path === "/api/log-settings") return json(settings);
    if (path.includes("/checkin")) return json(checkin(path.split("/")[3]));
    if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
    throw new Error("Unexpected local fixture endpoint");
  });
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });
const flush = async () => { await act(async () => { await Promise.resolve(); }); };
const tick = async (ms: number) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); };
async function mount() {
  const hook = renderHook(() => useManager());
  await flush();
  expect(hook.result.current.session).toBe("open");
  return hook;
}
const systemLogReads = () => fetchMock.mock.calls.filter(([path]) => path === "/api/system-logs").length;

describe("production manager async regressions (local fetch fixtures only)", () => {
  it("completes candidate polling with every read slower than 1000ms and no overlapping GETs", async () => {
    snapshot.operation = operation("login-1", "login", "running");
    const hook = await mount();
    let poll = 0;
    readAccounts = () => new Promise(resolve => setTimeout(() => {
      poll++;
      resolve(json({ ...base(), operation: operation("login-1", "login", poll < 2 ? "running" : "succeeded"),
        candidate: poll < 2 ? null : { id: "candidate", username: "fixture", upstream_user_id: "7", balance: account("a").balance, keys: [], expires_at: "2026-10-01T12:15:00Z" } }));
    }, 1600));
    await tick(1000);
    await tick(1600);
    await tick(1000);
    await tick(1600);
    expect(hook.result.current.state.candidate?.id).toBe("candidate");
    expect(hook.result.current.busy).toBe(false);
    expect(maximum).toBe(1);
    const stoppedAt = reads;
    await tick(10000);
    expect(reads).toBe(stoppedAt);
  });

  it("tracks accepted login across stale/null snapshots without storing the password or repeating POST", async () => {
    const hook = await mount();
    const credentials = { username: "fixture", password: "only-local-sentinel" };
    await act(async () => { await hook.result.current.addAccount(credentials); });
    expect(credentials.password).toBe("");
    expect(hook.result.current.busy).toBe(true);
    snapshot.operation = operation("older", "save", "succeeded");
    await tick(1000);
    expect(hook.result.current.busy).toBe(true);
    expect(hook.result.current.state.operation).toBeNull();
    snapshot = { ...base(), operation: operation("login-1", "login", "succeeded"), candidate: { id: "candidate", username: "fixture", upstream_user_id: "7", balance: account("a").balance, keys: [], expires_at: "2026-10-01T12:15:00Z" } };
    await tick(1000);
    expect(hook.result.current.state.candidate?.id).toBe("candidate");
    expect(hook.result.current.busy).toBe(false);
    expect(posts).toEqual(["/api/accounts/login"]);
    expect(JSON.stringify(hook.result.current)).not.toContain("only-local-sentinel");
  });

  it("deduplicates explicit reloads and queues a fresh snapshot after a concurrent POST", async () => {
    const hook = await mount();
    const old = deferred<Response>();
    readAccounts = () => old.promise;
    act(() => { void hook.result.current.load(); });
    let add!: Promise<void>;
    act(() => { add = hook.result.current.addAccount({ username: "fixture", password: "fixture" }); });
    await flush();
    expect(concurrent).toBe(1);
    snapshot.operation = operation("login-1", "login", "succeeded");
    readAccounts = async () => json(snapshot);
    await act(async () => { old.resolve(json(base())); await add; });
    expect(hook.result.current.state.operation?.id).toBe("login-1");
    expect(maximum).toBe(1);
  });

  it.each(["failed", "succeeded"] as const)("keeps refresh pending until matching %s and preserves account metadata", async status => {
    const hook = await mount();
    const original = snapshot.accounts;
    await act(async () => { await hook.result.current.refreshAccount("a"); });
    expect(hook.result.current.refreshingAccountId).toBe("a");
    snapshot.operation = operation("refresh-1", "refresh", "running", "a");
    await tick(1000);
    expect(hook.result.current.refreshingAccountId).toBe("a");
    snapshot.operation = { ...operation("refresh-1", "refresh", status, "a"), error: status === "failed" ? { code: "upstream_session_expired", message: "Session expired; log in again." } : null };
    await tick(1000);
    expect(hook.result.current.refreshingAccountId).toBeNull();
    expect(hook.result.current.state.accounts).toEqual(original);
    expect(hook.result.current.accountRefreshError.a).toBe(status === "failed" ? "Session expired; log in again." : undefined);
    expect(hook.result.current.notice).toBe("");
    snapshot.operation = operation("unrelated", "save", "succeeded");
    await act(async () => { await hook.result.current.load(); });
    expect(hook.result.current.accountRefreshError.a).toBe(status === "failed" ? "Session expired; log in again." : undefined);
  });

  it("stops missing-ID polling visibly and GET retry recovers without resubmitting", async () => {
    const hook = await mount();
    await act(async () => { await hook.result.current.refreshAccount("a"); });
    for (let i = 0; i < 15; i++) await tick(1000);
    expect(hook.result.current.loadError).toBe(true);
    expect(hook.result.current.notice).toContain("不要重复提交");
    expect(hook.result.current.state.operation).toBeNull();
    const stoppedAt = reads;
    await tick(10000);
    expect(reads).toBe(stoppedAt);
    snapshot.operation = operation("refresh-1", "refresh", "succeeded", "a");
    await act(async () => { await hook.result.current.load(); });
    expect(hook.result.current.refreshingAccountId).toBeNull();
    expect(hook.result.current.loadError).toBe(false);
    expect(posts).toEqual(["/api/accounts/a/refresh"]);
  });

  it("ignores late GET and accepted POST after logout", async () => {
    const hook = await mount();
    const get = deferred<Response>();
    const post = deferred<Response>();
    readAccounts = () => get.promise;
    custom = path => path.endsWith("/refresh") ? post.promise : undefined;
    let refresh!: Promise<void>;
    act(() => { void hook.result.current.load(); refresh = hook.result.current.refreshAccount("a"); });
    await act(async () => { await hook.result.current.logout(); });
    await act(async () => { get.resolve(json(snapshot)); post.resolve(json({ operation_id: "refresh-1" }, 202)); await refresh; });
    expect(hook.result.current.session).toBe("locked");
    expect(hook.result.current.state.accounts).toEqual([]);
    expect(hook.result.current.refreshingAccountId).toBeNull();
    const stoppedAt = reads;
    await tick(3000);
    expect(reads).toBe(stoppedAt);
  });

  it("StrictMode replay and unmount discard requests from earlier generations", async () => {
    const old = deferred<Response>();
    let count = 0;
    readAccounts = () => ++count === 1 ? old.promise : Promise.resolve(json(base()));
    const hook = renderHook(() => useManager(), { wrapper: StrictMode });
    await flush();
    expect(hook.result.current.session).toBe("open");
    await act(async () => { old.resolve(json({ ...base(), accounts: [] })); });
    expect(hook.result.current.state.accounts).toHaveLength(2);
    const late = deferred<Response>();
    readAccounts = () => late.promise;
    act(() => { void hook.result.current.load(); });
    hook.unmount();
    await act(async () => { late.resolve(json(base())); });
    const stoppedAt = reads;
    await tick(5000);
    expect(reads).toBe(stoppedAt);
  });

  it("checkin completion and stale details cannot switch the selected account back", async () => {
    const hook = await mount();
    const old = deferred<Response>();
    custom = path => path === "/api/accounts/a/checkin" ? old.promise : undefined;
    act(() => { hook.result.current.selectDetailAccount("a"); });
    await act(async () => { await hook.result.current.runAccountCheckin("a", { confirmRetry: false }); });
    act(() => { hook.result.current.selectDetailAccount("b"); });
    await flush();
    snapshot.operation = operation("checkin-1", "checkin", "succeeded", "a");
    await tick(1000);
    await act(async () => { old.resolve(json(checkin("a"))); });
    expect(hook.result.current.selectedAccountId).toBe("b");
    expect(hook.result.current.accountCheckin?.account_id).toBe("b");
    act(() => { hook.result.current.selectDetailAccount(null); });
    await act(async () => { await hook.result.current.loadAccountCheckin("a"); });
    expect(hook.result.current.accountCheckin).toBeNull();
  });

  it("latest system-log filters win out-of-order responses and loading state", async () => {
    const hook = await mount();
    const old = deferred<Response>();
    const latest = deferred<Response>();
    custom = path => path.includes("level=info") ? old.promise : path.includes("level=error") ? latest.promise : undefined;
    act(() => { void hook.result.current.loadSystemLogs({ level: "info" }); void hook.result.current.loadSystemLogs({ level: "error" }); });
    expect(hook.result.current.systemLogsLoading).toBe(true);
    await act(async () => { latest.resolve(json({ items: [], dropped_count: 22 })); });
    await act(async () => { old.resolve(json({ items: [], dropped_count: 11 })); });
    expect(hook.result.current.systemLogs?.dropped_count).toBe(22);
    expect(hook.result.current.systemLogsLoading).toBe(false);
    const stale = deferred<Response>();
    custom = path => path.includes("level=info") ? stale.promise : undefined;
    act(() => { void hook.result.current.loadSystemLogs({ level: "info" }); });
    await act(async () => { await hook.result.current.clearSystemLogs(); });
    await act(async () => { stale.resolve(json({ items: [], dropped_count: 99 })); });
    expect(hook.result.current.systemLogs?.dropped_count).toBe(0);
  });

  it.each([
    ["succeeded", false], ["failed", false],
    ["succeeded", true], ["failed", true],
  ] as const)("refreshes logs exactly once for %s login (immediate completion: %s)", async (status, immediate) => {
    const hook = await mount();
    const initialReads = systemLogReads();
    const terminal = operation("login-1", "login", status);
    if (immediate) snapshot.operation = terminal;
    await act(async () => { await hook.result.current.addAccount({ username: "fixture", password: "fixture" }); });
    if (!immediate) {
      expect(systemLogReads()).toBe(initialReads);
      snapshot.operation = operation("login-1", "login", "running");
      await tick(1000);
      expect(systemLogReads()).toBe(initialReads);
      snapshot.operation = terminal;
      await tick(1000);
    }
    expect(hook.result.current.state.operation?.status).toBe(status);
    expect(systemLogReads()).toBe(initialReads + 1);
    await act(async () => { await hook.result.current.load(); await hook.result.current.load(); });
    await tick(5000);
    expect(systemLogReads()).toBe(initialReads + 1);
    expect(posts).toEqual(["/api/accounts/login"]);
  });

  it.each(["succeeded", "failed"] as const)("refreshes once when an observed login becomes %s under StrictMode", async status => {
    snapshot.operation = operation("login-1", "login", "running");
    const hook = renderHook(() => useManager(), { wrapper: StrictMode });
    await flush();
    const initialReads = systemLogReads();
    snapshot.operation = operation("login-1", "login", status);
    await tick(1000);
    expect(systemLogReads()).toBe(initialReads + 1);
    await act(async () => { await hook.result.current.load(); });
    await tick(3000);
    expect(systemLogReads()).toBe(initialReads + 1);
    expect(posts).toEqual([]);
  });

  it.each(["refresh", "save", "select_key", "checkin"] as const)("does not refresh system logs for terminal %s", async kind => {
    snapshot.operation = operation("other-1", kind, "running", "a");
    await mount();
    const initialReads = systemLogReads();
    snapshot.operation = operation("other-1", kind, "succeeded", "a");
    await tick(1000);
    expect(systemLogReads()).toBe(initialReads);
  });

  it.each(["http", "network"] as const)("keeps prior logs and the original login failure when log refresh fails (%s)", async failure => {
    const previous = systemPage("previous-log");
    custom = path => path === "/api/system-logs" ? Promise.resolve(json(previous)) : undefined;
    const hook = await mount();
    expect(hook.result.current.systemLogs).toEqual(previous);
    custom = path => path === "/api/system-logs"
      ? failure === "http" ? Promise.resolve(json({ error: { code: "logging_unavailable", message: "Logging unavailable." } }, 503))
        : Promise.reject(new TypeError("Local mock network failure"))
      : undefined;
    const originalError = { code: "upstream_timeout", message: "上游浏览器登录流程超时。" };
    snapshot.operation = { ...operation("login-1", "login", "failed"), error: originalError };
    await act(async () => { await hook.result.current.addAccount({ username: "fixture", password: "fixture" }); });
    expect(hook.result.current.state.operation?.error).toEqual(originalError);
    expect(hook.result.current.systemLogs).toEqual(previous);
    expect(hook.result.current.systemLogsError).toBe("无法读取系统日志，请检查后端状态后重试");
    expect(hook.result.current.systemLogsLoading).toBe(false);
    expect(hook.result.current.notice).toBe("");
    expect(hook.result.current.session).toBe("open");
    custom = path => path === "/api/system-logs" ? Promise.resolve(json({ items: [], dropped_count: 0 })) : undefined;
    await act(async () => { await hook.result.current.loadSystemLogs(); });
    expect(hook.result.current.systemLogs).toEqual({ items: [], dropped_count: 0 });
    expect(hook.result.current.systemLogsError).toBeNull();
    expect(hook.result.current.state.operation?.error).toEqual(originalError);
  });

  it("still locks the session on a current system-log 401", async () => {
    const hook = await mount();
    custom = path => path === "/api/system-logs" ? Promise.resolve(json(null, 401)) : undefined;
    await act(async () => { await hook.result.current.loadSystemLogs(); });
    expect(hook.result.current.session).toBe("locked");
    expect(hook.result.current.systemLogs).toBeNull();
    expect(hook.result.current.systemLogsError).toBeNull();
  });

  it.each([false, true])("late login completion cannot refresh logs after logout (replacement session: %s)", async reopen => {
    const hook = await mount();
    await act(async () => { await hook.result.current.addAccount({ username: "fixture", password: "fixture" }); });
    const late = deferred<Response>();
    readAccounts = () => late.promise;
    act(() => { void hook.result.current.load(); });
    await act(async () => { await hook.result.current.logout(); });
    if (reopen) {
      snapshot = base();
      readAccounts = async () => json(snapshot);
      await act(async () => { await hook.result.current.unlock("only-local-root-fixture"); });
    }
    const callsBeforeLate = fetchMock.mock.calls.length;
    const logsBeforeLate = hook.result.current.systemLogs;
    await act(async () => { late.resolve(json({ ...base(), operation: operation("login-1", "login", "failed") })); });
    await tick(3000);
    expect(fetchMock.mock.calls).toHaveLength(callsBeforeLate);
    expect(hook.result.current.systemLogs).toEqual(logsBeforeLate);
    expect(hook.result.current.systemLogsError).toBeNull();
    expect(hook.result.current.state.operation).toBeNull();
    expect(hook.result.current.session).toBe(reopen ? "open" : "locked");
  });

  it("late login acceptance after logout cannot read accounts or logs", async () => {
    const hook = await mount();
    const late = deferred<Response>();
    custom = path => path === "/api/accounts/login" ? late.promise : undefined;
    let add!: Promise<void>;
    act(() => { add = hook.result.current.addAccount({ username: "fixture", password: "fixture" }); });
    await act(async () => { await hook.result.current.logout(); });
    const callsBeforeLate = fetchMock.mock.calls.length;
    await act(async () => { late.resolve(json({ operation_id: "login-1" }, 202)); await add; });
    await tick(3000);
    expect(fetchMock.mock.calls).toHaveLength(callsBeforeLate);
    expect(hook.result.current.session).toBe("locked");
    expect(hook.result.current.systemLogs).toBeNull();
  });

  it.each(["success", "http", "network", "unauthorized"] as const)("discards old-session terminal-login log responses (%s)", async outcome => {
    const hook = await mount();
    const late = deferred<Response>();
    let oldSignal: AbortSignal | null | undefined;
    custom = (path, init) => {
      if (path !== "/api/system-logs") return undefined;
      oldSignal = init.signal;
      return late.promise;
    };
    snapshot.operation = operation("login-1", "login", "failed");
    await act(async () => { await hook.result.current.addAccount({ username: "fixture", password: "fixture" }); });
    expect(hook.result.current.systemLogsLoading).toBe(true);
    await act(async () => { await hook.result.current.logout(); });
    expect(oldSignal?.aborted).toBe(true);
    snapshot = base();
    const current = systemPage("current-session");
    custom = path => path === "/api/system-logs" ? Promise.resolve(json(current)) : undefined;
    await act(async () => { await hook.result.current.unlock("only-local-root-fixture"); });
    expect(hook.result.current.systemLogs).toEqual(current);
    const callsBeforeLate = fetchMock.mock.calls.length;
    await act(async () => {
      if (outcome === "network") late.reject(new TypeError("Local mock network failure"));
      else late.resolve(json(systemPage("old-session"), outcome === "http" ? 503 : outcome === "unauthorized" ? 401 : 200));
    });
    expect(fetchMock.mock.calls).toHaveLength(callsBeforeLate);
    expect(hook.result.current.systemLogs).toEqual(current);
    expect(hook.result.current.systemLogsError).toBeNull();
    expect(hook.result.current.systemLogsLoading).toBe(false);
    expect(hook.result.current.session).toBe("open");
  });

  it("settings save cannot be overwritten by an earlier settings GET", async () => {
    const hook = await mount();
    const old = deferred<Response>();
    custom = (path, init) => path === "/api/log-settings" ? init.method === "PUT" ? Promise.resolve(json({ ...settings, level: "debug" })) : old.promise : undefined;
    act(() => { void hook.result.current.loadLogSettings(); });
    await act(async () => { await hook.result.current.saveLogSettings({ ...settings, level: "debug" }); });
    await act(async () => { old.resolve(json(settings)); });
    expect(hook.result.current.logSettings?.level).toBe("debug");
  });

  it("bounds a continuously running accepted operation without inventing remote failure", async () => {
    const hook = await mount();
    snapshot.operation = operation("refresh-1", "refresh", "running", "a");
    await act(async () => { await hook.result.current.refreshAccount("a"); });
    await tick(180000);
    expect(hook.result.current.loadError).toBe(true);
    expect(hook.result.current.notice).toContain("无法确认任务状态");
    expect(hook.result.current.state.operation?.status).toBe("running");
    expect(hook.result.current.accountRefreshError.a).toBeUndefined();
    const stoppedAt = reads;
    await tick(5000);
    expect(reads).toBe(stoppedAt);
    expect(posts).toHaveLength(1);
  });

  it("bounds a stalled status fetch, keeps old data and recovers by GET only", async () => {
    const hook = await mount();
    readAccounts = () => new Promise((_resolve, reject) => {
      const request = [...fetchMock.mock.calls].reverse().find(call => call[0] === "/api/accounts");
      const signal = request?.[1]?.signal as AbortSignal;
      signal.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")), { once: true });
    });
    let refresh!: Promise<void>;
    act(() => { refresh = hook.result.current.refreshAccount("a"); });
    await flush();
    await tick(15000);
    await act(async () => { await refresh; });
    expect(hook.result.current.loadError).toBe(true);
    expect(hook.result.current.state.accounts).toHaveLength(2);
    expect(hook.result.current.refreshingAccountId).toBe("a");
    snapshot.operation = operation("refresh-1", "refresh", "succeeded", "a");
    readAccounts = async () => json(snapshot);
    await act(async () => { await hook.result.current.load(); });
    expect(hook.result.current.refreshingAccountId).toBeNull();
    expect(maximum).toBe(1);
    expect(posts).toHaveLength(1);
  });

  it("a confirmed route response is not undone by an older accounts GET", async () => {
    snapshot.accounts[1].selected_key = { id: "key", name: "fixture", masked: "........" };
    const hook = await mount();
    const old = deferred<Response>();
    readAccounts = () => old.promise;
    custom = path => path.endsWith("/route") ? Promise.resolve(json({ ...snapshot, route_account_id: "b" })) : undefined;
    act(() => { void hook.result.current.load(); });
    await act(async () => { await hook.result.current.routeAccount("b"); });
    await act(async () => { old.resolve(json(snapshot)); });
    expect(hook.result.current.state.route_account_id).toBe("b");
    expect(hook.result.current.pendingRouteAccountId).toBeNull();
    expect(hook.result.current.selectedAccountId).toBeNull();
  });

  it.each(["save", "select_key"] as const)("tracks accepted %s across the first absent operation", async kind => {
    const hook = await mount();
    await act(async () => {
      if (kind === "save") await hook.result.current.saveCandidate("candidate", null);
      else await hook.result.current.selectAccountKey("a", "key");
    });
    expect(hook.result.current.busy).toBe(true);
    snapshot.operation = operation(kind === "save" ? "save-1" : "key-1", kind, "succeeded", kind === "save" ? null : "a");
    await tick(1000);
    expect(hook.result.current.busy).toBe(false);
    expect(posts).toHaveLength(1);
  });

  it("keeps login rejection modal-local, while reveal 401 does not lock the session", async () => {
    const hook = await mount();
    custom = path => path === "/api/accounts/login" || path.endsWith("/key/reveal") ? Promise.resolve(json({ error: { code: "unauthorized", message: "fixture" } }, 401)) : undefined;
    await act(async () => { await expect(hook.result.current.addAccount({ username: "fixture", password: "fixture" })).rejects.toThrow(); });
    expect(hook.result.current.notice).toBe("");
    await act(async () => { await expect(hook.result.current.revealKey("a", "only-local-root-fixture")).rejects.toThrow("root"); });
    expect(hook.result.current.session).toBe("open");
    readAccounts = async () => json(null, 401);
    await act(async () => { await hook.result.current.load(); });
    expect(hook.result.current.session).toBe("locked");
  });
});
