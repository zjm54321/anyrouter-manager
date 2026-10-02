import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountsResponse, OperationDTO } from "./api";
import { useManager } from "./useManager";

const minute = 60_000;
const preferenceKey = "anyrouter-manager.quota-auto-refresh.v1";
const json = (value: unknown, status = 200) => new Response(status === 204 ? null : JSON.stringify(value),
  { status, headers: { "Content-Type": "application/json" } });
let snapshot: AccountsResponse;
let posts: string[];
let response: () => Promise<Response>;
let reads: () => Promise<Response>;
let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;
let visibility: "visible" | "hidden";
function operation(id: string, status: OperationDTO["status"], accountId = "a", code?: string): OperationDTO {
  return { id, kind: "refresh", status, account_id: accountId, phase: "done",
    error: code ? { code, message: "Synthetic refresh failure" } : null };
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-01T12:00:00Z"));
  localStorage.clear();
  visibility = "visible";
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility);
  snapshot = { route_account_id: "b", candidate: null, operation: null, accounts: ["a", "b"].map(id => ({
    id, revision: "1", username: "synthetic", upstream_user_id: "7", keys: [], selected_key: null,
    added_at: new Date().toISOString(), balance: { quota_raw: "500000", used_quota_raw: "0",
      fetched_at: "2026-09-01T00:00:00Z" },
  })) };
  posts = [];
  reads = async () => json(snapshot);
  response = async () => json({ operation_id: "refresh-" + posts.length }, 202);
  fetchMock = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const path = String(input);
    if (path === "/api/accounts") return reads();
    if (path.endsWith("/refresh")) { posts.push(path); return response(); }
    if (path === "/api/admin/session") return json(null, 204);
    if (path === "/api/checkin") return json({ settings: { enabled: false, time: "09:00", interval_minutes: 30,
      timezone: "Asia/Shanghai", reset_time: "08:00" }, cycle_date: "2026-10-01", next_run_at: null, schedule: [] });
    if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
    if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
    throw new Error("Unexpected synthetic API request");
  });
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); vi.unstubAllGlobals(); });
const flush = async () => { await act(async () => { await Promise.resolve(); }); };
const tick = async (ms: number) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); };
async function mount() {
  const hook = renderHook(() => useManager());
  await flush();
  return hook;
}
async function visible(next: "visible" | "hidden") {
  await act(async () => { visibility = next; document.dispatchEvent(new Event("visibilitychange")); });
}
async function finish(hook: Awaited<ReturnType<typeof mount>>, status: OperationDTO["status"] = "succeeded", code?: string) {
  const accountId = posts.at(-1)!.split("/")[3];
  snapshot = { ...snapshot, operation: operation("refresh-" + posts.length, status, accountId, code) };
  if (status === "succeeded") snapshot.accounts = snapshot.accounts.map(account => account.id === accountId
    ? { ...account, revision: account.revision + "1", balance: { ...account.balance, fetched_at: new Date().toISOString() } } : account);
  await act(async () => { await hook.result.current.load(); });
  await tick(0);
}

describe("page-local quota auto refresh (offline fetch fixtures)", () => {
  it("defaults enabled/10, waits first interval, switches to 5 and persists harmless preferences", async () => {
    const hook = await mount();
    expect(hook.result.current.quotaAutoRefreshEnabled).toBe(true);
    expect(hook.result.current.quotaRefreshIntervalMinutes).toBe(10);
    await tick(4 * minute);
    expect(posts).toEqual([]);
    act(() => { hook.result.current.setQuotaRefreshIntervalMinutes(5); });
    await tick(minute);
    expect(posts).toEqual(["/api/accounts/a/refresh"]);
    act(() => { hook.result.current.setQuotaAutoRefreshEnabled(false); });
    await finish(hook);
    await tick(10 * minute);
    expect(posts).toHaveLength(1);
    expect(JSON.parse(localStorage.getItem(preferenceKey)!)).toEqual({ enabled: false, minutes: 5 });
    expect(Object.keys(localStorage)).toEqual([preferenceKey]);
    hook.unmount();
    const reopened = await mount();
    expect(reopened.result.current.quotaAutoRefreshEnabled).toBe(false);
    expect(reopened.result.current.quotaRefreshIntervalMinutes).toBe(5);
  });

  it("serializes until exact terminal ID/account, not acceptance, and preserves manual refresh override", async () => {
    const hook = await mount();
    await tick(10 * minute);
    expect(posts).toHaveLength(1);
    await act(async () => { await hook.result.current.refreshAccount("b"); });
    snapshot.operation = operation("foreign", "succeeded");
    await tick(1000);
    expect(posts).toHaveLength(1);
    snapshot.operation = operation("refresh-1", "succeeded", "b");
    await tick(1000);
    expect(posts).toHaveLength(1);
    await finish(hook);
    expect(posts).toEqual(["/api/accounts/a/refresh", "/api/accounts/b/refresh"]);
    await finish(hook);
    act(() => { hook.result.current.setQuotaAutoRefreshEnabled(false); });
    await act(async () => { await hook.result.current.refreshAccount("a"); });
    expect(posts).toHaveLength(3);
    expect(hook.result.current.state.route_account_id).toBe("b");
  });

  it.each(["login", "checkin"] as const)("blocks global %s busy and same-render duplicate manual submissions", async kind => {
    snapshot.operation = { ...operation("busy", "running"), kind };
    const hook = await mount();
    await tick(10 * minute);
    expect(posts).toEqual([]);
    snapshot.operation = { ...operation("busy", "succeeded"), kind };
    await act(async () => { await hook.result.current.load(); });
    act(() => { void hook.result.current.refreshAccount("a"); void hook.result.current.refreshAccount("b"); });
    await flush();
    await tick(0);
    expect(posts).toEqual(["/api/accounts/a/refresh"]);
  });

  it("hides new work and reconciles GET before resuming a due refresh", async () => {
    const hook = await mount();
    await visible("hidden");
    await tick(10 * minute);
    expect(posts).toEqual([]);
    let resolve!: (value: Response) => void;
    const deferred = new Promise<Response>(done => { resolve = done; });
    reads = () => deferred;
    await visible("visible");
    await tick(0);
    expect(posts).toEqual([]);
    reads = async () => json(snapshot);
    await act(async () => { resolve(json(snapshot)); });
    await tick(0);
    expect(posts).toHaveLength(1);
    await visible("hidden");
    await finish(hook);
    expect(posts).toHaveLength(1);
    await visible("visible");
    await tick(0);
    expect(posts).toHaveLength(2);
  });

  it("backoffs failures without changing balance, pauses expired sessions until manual success", async () => {
    const hook = await mount();
    await tick(10 * minute);
    const oldBalance = hook.result.current.state.accounts[0].balance;
    await finish(hook, "failed", "upstream_session_expired");
    expect(posts).toHaveLength(2);
    expect(hook.result.current.state.accounts[0].balance).toEqual(oldBalance);
    await finish(hook, "failed", "upstream_unavailable");
    await tick(9 * minute);
    expect(posts).toHaveLength(2);
    await tick(minute);
    expect(posts.at(-1)).toBe("/api/accounts/b/refresh");
    await finish(hook);
    await act(async () => { await hook.result.current.refreshAccount("a"); });
    expect(posts.at(-1)).toBe("/api/accounts/a/refresh");
    await finish(hook);
    await tick(10 * minute);
    expect(posts.slice(4)).toContain("/api/accounts/a/refresh");
  });

  it("locks on management 401 and drops accepted POST after logout/unmount", async () => {
    response = async () => json({ error: { code: "unauthorized", message: "Synthetic unauthorized" } }, 401);
    let hook = await mount();
    await tick(10 * minute);
    expect(hook.result.current.session).toBe("locked");
    await tick(20 * minute);
    expect(posts).toHaveLength(1);
    hook.unmount();
    let resolve!: (value: Response) => void;
    response = () => new Promise(done => { resolve = done; });
    hook = await mount();
    await tick(10 * minute);
    const signal = [...fetchMock.mock.calls].reverse().find(([path]) => String(path).endsWith("/refresh"))![1]!.signal as AbortSignal;
    await act(async () => { await hook.result.current.logout(); });
    expect(signal.aborted).toBe(true);
    await act(async () => { resolve(json({ operation_id: "late" }, 202)); });
    expect(hook.result.current.session).toBe("locked");
    expect(hook.result.current.refreshingAccountId).toBeNull();
    const count = posts.length;
    hook.unmount();
    await tick(20 * minute);
    expect(posts).toHaveLength(count);
  });

  it("pauses unknown status, respects future fetched_at, and tolerates unavailable preference storage", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("unavailable"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("unavailable"); });
    snapshot.accounts = [snapshot.accounts[0]];
    snapshot.accounts[0].balance.fetched_at = new Date(Date.now() + 10 * minute).toISOString();
    const hook = await mount();
    await tick(10 * minute);
    expect(posts).toEqual([]);
    await tick(10 * minute);
    expect(posts).toHaveLength(1);
    await tick(20 * minute);
    expect(hook.result.current.loadError).toBe(true);
    expect(posts).toHaveLength(1);
    expect(fetchMock.mock.calls.filter(([, init]) => init?.method === "POST").map(([path]) => path))
      .toEqual(["/api/accounts/a/refresh"]);
  });

  it("pauses lost acceptance rather than retrying and discards an old session result after reopening", async () => {
    response = async () => { throw new TypeError("Synthetic connection lost"); };
    const hook = await mount();
    await tick(10 * minute);
    expect(hook.result.current.loadError).toBe(true);
    await tick(20 * minute);
    expect(posts).toHaveLength(1);
    await act(async () => { await hook.result.current.load(); });
    let resolve!: (value: Response) => void;
    response = () => new Promise(done => { resolve = done; });
    act(() => { void hook.result.current.refreshAccount("a"); });
    await flush();
    await act(async () => { await hook.result.current.logout(); });
    await act(async () => { await hook.result.current.unlock("synthetic-local-only"); });
    await act(async () => { resolve(json({ operation_id: "previous-session" }, 202)); });
    expect(hook.result.current.session).toBe("open");
    expect(hook.result.current.busy).toBe(false);
    expect(hook.result.current.refreshingAccountId).toBeNull();
    const count = posts.length;
    act(() => { void hook.result.current.refreshAccount("b"); });
    await flush();
    const signal = [...fetchMock.mock.calls].reverse().find(([path]) => String(path).endsWith("/refresh"))![1]!.signal as AbortSignal;
    hook.unmount();
    expect(signal.aborted).toBe(true);
    await act(async () => { resolve(json({ operation_id: "unmounted" }, 202)); });
    await tick(20 * minute);
    expect(posts).toHaveLength(count + 1);
  });

  it("backs off rejected POSTs and gives other due accounts a turn without catch-up loops", async () => {
    response = async () => json({ error: { code: "upstream_unavailable", message: "Synthetic unavailable" } }, 502);
    const hook = await mount();
    await tick(10 * minute);
    await tick(0);
    expect(posts).toEqual(["/api/accounts/a/refresh", "/api/accounts/b/refresh"]);
    await tick(9 * minute);
    expect(posts).toHaveLength(2);
    await tick(minute);
    await tick(0);
    expect(posts).toEqual(["/api/accounts/a/refresh", "/api/accounts/b/refresh",
      "/api/accounts/a/refresh", "/api/accounts/b/refresh"]);
    expect(hook.result.current.state.route_account_id).toBe("b");
  });

  it("uses the selected interval after failure and resumes after showing a locked page then unlocking", async () => {
    snapshot.accounts = [snapshot.accounts[0]];
    localStorage.setItem(preferenceKey, JSON.stringify({ enabled: true, minutes: 5 }));
    const hook = await mount();
    await tick(5 * minute);
    await finish(hook, "failed", "upstream_unavailable");
    act(() => { hook.result.current.setQuotaRefreshIntervalMinutes(10); });
    await tick(5 * minute);
    expect(posts).toHaveLength(1);
    await tick(5 * minute);
    expect(posts).toHaveLength(2);
    await finish(hook);
    await act(async () => { await hook.result.current.logout(); });
    await visible("hidden");
    await visible("visible");
    await act(async () => { await hook.result.current.unlock("synthetic-local-only"); });
    await tick(10 * minute);
    expect(posts).toHaveLength(3);
  });

  it.each([
    ["login", "auto"], ["login", "manual"],
    ["checkin", "auto"], ["checkin", "manual"],
  ] as const)("serializes %s against auto refresh with %s admitted first before rerender", async (kind, first) => {
    snapshot.accounts = [snapshot.accounts[0]];
    const hook = await mount();
    const original = fetchMock.getMockImplementation()!;
    const submitted: string[] = [];
    let resolve!: (value: Response) => void;
    fetchMock.mockImplementation((input: RequestInfo | URL, init: RequestInit = {}) => {
      if (init.method === "POST" && String(input) !== "/api/admin/session") {
        submitted.push(String(input));
        return new Promise<Response>(done => { resolve = done; });
      }
      return original(input, init);
    });
    const manual = () => kind === "login"
      ? hook.result.current.addAccount({ username: "synthetic", password: "synthetic" })
      : hook.result.current.runAccountCheckin("a", { confirmRetry: false });
    act(() => {
      if (first === "manual") void manual();
      vi.advanceTimersByTime(10 * minute);
      if (first === "auto") void manual();
    });
    await flush();
    expect(submitted).toEqual([first === "auto" ? "/api/accounts/a/refresh"
      : kind === "login" ? "/api/accounts/login" : "/api/accounts/a/checkin/run"]);
    expect(hook.result.current.busy).toBe(true);
    expect(hook.result.current.sending).toBe(true);
    await act(async () => { resolve(json({ operation_id: "owner" }, 202)); });
    expect(hook.result.current.busy).toBe(true);
    const ownerKind = first === "auto" ? "refresh" : kind;
    const terminal: OperationDTO = { ...operation("owner", "succeeded"), kind: ownerKind };
    // Neither a different ID/kind nor a different tracked account is completion.
    for (const wrong of [{ ...terminal, id: "foreign" }, { ...terminal, kind: "save" as const },
      ...(ownerKind === "login" ? [] : [{ ...terminal, account_id: "b" }])]) {
      snapshot.operation = wrong;
      await act(async () => { await hook.result.current.load(); await manual();
        await hook.result.current.refreshAccount("a"); });
      expect(submitted).toHaveLength(1);
      expect(hook.result.current.busy).toBe(true);
    }
    snapshot.operation = terminal;
    await act(async () => { await hook.result.current.load(); });
    expect(hook.result.current.busy).toBe(false);
    if (first === "auto") act(() => { void manual(); });
    else await tick(0);
    expect(submitted).toHaveLength(2);
    expect(hook.result.current.busy).toBe(true);
  });

  it("keeps a new session owner when a pre-logout submission finishes late", async () => {
    const hook = await mount();
    const original = fetchMock.getMockImplementation()!;
    const resolves: ((value: Response) => void)[] = [];
    const signals: AbortSignal[] = [];
    fetchMock.mockImplementation((input: RequestInfo | URL, init: RequestInit = {}) => {
      if (String(input) === "/api/accounts/login") {
        signals.push(init.signal as AbortSignal);
        return new Promise<Response>(resolve => { resolves.push(resolve); });
      }
      return original(input, init);
    });
    act(() => { void hook.result.current.addAccount({ username: "synthetic", password: "synthetic" }); });
    await act(async () => { await hook.result.current.logout(); await hook.result.current.unlock("synthetic"); });
    expect(signals[0].aborted).toBe(true);
    act(() => { void hook.result.current.addAccount({ username: "synthetic", password: "synthetic" }); });
    await act(async () => { resolves[0](json({ operation_id: "old-owner" }, 202)); });
    expect(hook.result.current.sending).toBe(true);
    await act(async () => { await hook.result.current.refreshAccount("a"); });
    expect(posts).toEqual([]);
    await act(async () => { resolves[1](json({ operation_id: "new-owner" }, 202)); });
    expect(hook.result.current.busy).toBe(true);
    snapshot.operation = { ...operation("new-owner", "succeeded"), kind: "login" };
    await act(async () => { await hook.result.current.load(); await hook.result.current.refreshAccount("a"); });
    expect(posts).toHaveLength(1);
  });

  it("releases only rejected or matching save/key owners, and immediate already-recorded checkin", async () => {
    const hook = await mount();
    const original = fetchMock.getMockImplementation()!;
    const submitted: string[] = [];
    let reply = json({ error: { code: "candidate_expired", message: "Synthetic expired" } }, 409);
    fetchMock.mockImplementation((input: RequestInfo | URL, init: RequestInit = {}) => {
      if (init.method === "POST" && String(input) !== "/api/admin/session") {
        submitted.push(String(input));
        return Promise.resolve(reply);
      }
      return original(input, init);
    });
    await act(async () => {
      await expect(hook.result.current.saveCandidate("synthetic-candidate", null)).rejects.toThrow();
    });
    expect(hook.result.current.candidateExpired).toBe(true);
    expect(hook.result.current.busy).toBe(false);
    for (const kind of ["save", "select_key"] as const) {
      reply = json({ operation_id: kind }, 202);
      await act(async () => {
        if (kind === "save") await hook.result.current.saveCandidate("synthetic-candidate", null);
        else await hook.result.current.selectAccountKey("a", "synthetic-key");
      });
      const count = submitted.length;
      await act(async () => { await hook.result.current.refreshAccount("b"); });
      expect(submitted).toHaveLength(count);
      expect(hook.result.current.busy).toBe(true);
      snapshot.operation = { ...operation(kind, "succeeded"), kind };
      await act(async () => { await hook.result.current.load(); });
      expect(hook.result.current.busy).toBe(false);
    }
    reply = json({ already_recorded: true });
    await act(async () => { await hook.result.current.runAccountCheckin("a", { confirmRetry: false }); });
    expect(hook.result.current.busy).toBe(false);
    reply = json({ operation_id: "after-immediate" }, 202);
    await act(async () => { await hook.result.current.refreshAccount("b"); });
    expect(submitted.at(-1)).toBe("/api/accounts/b/refresh");
    expect(hook.result.current.state.route_account_id).toBe("b");
  });
});
