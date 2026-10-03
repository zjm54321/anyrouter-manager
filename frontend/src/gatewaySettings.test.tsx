import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useManager } from "./useManager";

function json(data: unknown, status = 200): Response {
  return new Response(status === 204 ? null : JSON.stringify(data), {
    status,
    headers: status === 204 ? {} : { "Content-Type": "application/json" },
  });
}

describe("useManager - Gateway Settings Lifecycle", () => {
  const fetchMock = vi.fn();

  beforeEach(() => {
    fetchMock.mockReset();
    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";

      if (path === "/api/admin/session") {
        return json(null, 204);
      }
      if (path === "/api/accounts") {
        return json({ accounts: [], route_account_id: null });
      }
      if (path === "/api/checkin") {
        return json({
          settings: {
            enabled: false,
            time: "09:00",
            interval_minutes: 30,
            timezone: "Asia/Shanghai",
            reset_time: "08:00",
          },
          cycle_date: "2026-10-01",
          next_run_at: null,
          schedule: [],
        });
      }
      if (path === "/api/log-settings") {
        return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      }
      if (path.includes("logs")) {
        return json({ items: [], dropped_count: 0 });
      }
      if (path === "/api/gateway-settings") {
        if (method === "GET") {
          return json({ responses_mode: "pass" });
        }
        if (method === "PUT") {
          const body = JSON.parse(String(init?.body || "{}"));
          return json({ responses_mode: body.responses_mode });
        }
      }
      throw new Error(`Unexpected endpoint in mock: ${path} [${method}]`);
    });
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  async function mount() {
    const hook = renderHook(() => useManager());
    await act(async () => {
      await Promise.resolve();
    });
    return hook;
  }

  it("loads confirmed gateway settings on loadGatewaySettings call", async () => {
    const hook = await mount();

    // Not automatically loaded on session open (no unrequested API calls)
    expect(hook.result.current.gatewaySettings).toBeNull();
    expect(hook.result.current.gatewaySettingsError).toBeNull();

    // Call loadGatewaySettings
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });

    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });
    expect(hook.result.current.gatewaySettingsError).toBeNull();
  });

  it("handles 404 response gracefully by setting explicit error without infinite retries", async () => {
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const path = String(input);
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        return json({ error: "not found" }, 404);
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });

    expect(hook.result.current.gatewaySettings).toBeNull();
    expect(hook.result.current.gatewaySettingsError).toBe("网关设置功能不可用（服务端未启用）");
  });

  it("handles 401 response by locking the session", async () => {
    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const path = String(input);
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        return json({ error: "unauthorized" }, 401);
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });

    expect(hook.result.current.session).toBe("locked");
    expect(hook.result.current.gatewaySettings).toBeNull();
  });

  it("saves gateway settings with exact payload and updates state without triggering model/account calls", async () => {
    const hook = await mount();

    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });

    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });

    // Save with "adapt"
    await act(async () => {
      await hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
    });

    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "adapt" });

    // Verify PUT request payload
    const putCalls = fetchMock.mock.calls.filter(
      call => String(call[0]) === "/api/gateway-settings" && call[1]?.method === "PUT"
    );
    expect(putCalls).toHaveLength(1);
    expect(JSON.parse(putCalls[0][1].body)).toEqual({ responses_mode: "adapt" });

    // Verify NO account refresh or checkin calls were made during save
    const refreshCalls = fetchMock.mock.calls.filter(call => String(call[0]).includes("refresh"));
    expect(refreshCalls).toHaveLength(0);
  });

  it("discards out-of-order responses so a stale slow load does not overwrite newer settings", async () => {
    let slowResolve: ((res: Response) => void) | null = null;
    let fastResolve: ((res: Response) => void) | null = null;
    let callCount = 0;

    fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
      const path = String(input);
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        callCount++;
        if (callCount === 1) {
          return new Promise<Response>(resolve => {
            slowResolve = resolve;
          });
        }
        return new Promise<Response>(resolve => {
          fastResolve = resolve;
        });
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // Trigger first request (slow)
    act(() => {
      void hook.result.current.loadGatewaySettings();
    });

    // Trigger second request (fast)
    act(() => {
      void hook.result.current.loadGatewaySettings();
    });

    // Resolve second request first with "auto"
    await act(async () => {
      fastResolve!(json({ responses_mode: "auto" }));
      await Promise.resolve();
    });

    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "auto" });

    // Now resolve the first request late with "pass"
    await act(async () => {
      slowResolve!(json({ responses_mode: "pass" }));
      await Promise.resolve();
    });

    // Must still be "auto" because first request sequence is older!
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "auto" });
  });

  it("does not allow an older GET to overwrite a newer confirmed PUT", async () => {
    let getResolve: ((res: Response) => void) | null = null;

    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        if (method === "GET") {
          return new Promise<Response>(resolve => {
            getResolve = resolve;
          });
        }
        if (method === "PUT") {
          const body = JSON.parse(String(init?.body || "{}"));
          return json({ responses_mode: body.responses_mode });
        }
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // 1. Slow GET started
    act(() => {
      void hook.result.current.loadGatewaySettings();
    });

    // 2. User saves "adapt" via PUT (higher sequence number)
    await act(async () => {
      await hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
    });

    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "adapt" });

    // 3. Late GET resolves with "pass"
    await act(async () => {
      getResolve!(json({ responses_mode: "pass" }));
      await Promise.resolve();
    });

    // Stale GET must NOT overwrite confirmed PUT!
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "adapt" });
  });

  it("save completing after logout/new session does not update confirmed state or lock new session", async () => {
    let putResolve: ((res: Response) => void) | null = null;

    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") {
        if (method === "DELETE") return json(null, 204);
        return json(null, 204);
      }
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings" && method === "PUT") {
        return new Promise<Response>(resolve => {
          putResolve = resolve;
        });
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // Start saving in initial session
    let savePromise: Promise<unknown>;
    act(() => {
      savePromise = hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
    });

    // User logs out while save is in flight (advances epoch generation)
    await act(async () => {
      await hook.result.current.logout();
    });

    expect(hook.result.current.session).toBe("locked");
    expect(hook.result.current.gatewaySettings).toBeNull();

    // Now late PUT returns 401
    let saveError: Error | null = null;
    await act(async () => {
      putResolve!(json({ error: "unauthorized" }, 401));
      try {
        await savePromise;
      } catch (err) {
        saveError = err as Error;
      }
    });

    // Caller observes rejection, not resolved success!
    expect(saveError).toBeInstanceOf(Error);
    expect((saveError as unknown as Error).message).toMatch(/管理请求未通过鉴权|保存操作已被取消|已取消/);

    // State remains clean locked, no error corruption
    expect(hook.result.current.session).toBe("locked");
    expect(hook.result.current.gatewaySettings).toBeNull();
  });

  it("rejects when server returns valid enum mode that differs from requested mode, preserving last confirmed state", async () => {
    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        if (method === "GET") return json({ responses_mode: "pass" });
        if (method === "PUT") {
          // Server returns a valid enum mode, but DIFFERENT from requested "adapt" (returns "pass")
          return json({ responses_mode: "pass" });
        }
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // 1. Initial confirmed load is "pass"
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });

    // 2. Request save to "adapt", server returns valid "pass" (mismatched)
    let mismatchError: Error | null = null;
    await act(async () => {
      try {
        await hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
      } catch (err) {
        mismatchError = err as Error;
      }
    });

    expect(mismatchError).toBeInstanceOf(Error);
    expect((mismatchError as unknown as Error).message).toBe("服务端确认的转发模式与请求不一致");

    // Confirmed state MUST remain last confirmed "pass", NOT corrupted
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });
  });

  it("handles genuine reopened session: old in-flight PUT returning 401 does not lock new session or clear new session save guard", async () => {
    let oldPutResolve: ((res: Response) => void) | null = null;
    let newPutResolve: ((res: Response) => void) | null = null;
    let putCount = 0;

    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        if (method === "GET") return json({ responses_mode: "pass" });
        if (method === "PUT") {
          putCount++;
          if (putCount === 1) {
            return new Promise<Response>(resolve => {
              oldPutResolve = resolve;
            });
          }
          return new Promise<Response>(resolve => {
            newPutResolve = resolve;
          });
        }
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // 1. Initial session: load confirmed "pass"
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });

    // 2. Start old PUT in session 1
    let oldSavePromise: Promise<unknown>;
    act(() => {
      oldSavePromise = hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
    });
    expect(putCount).toBe(1);

    // 3. User logs out
    await act(async () => {
      await hook.result.current.logout();
    });
    expect(hook.result.current.session).toBe("locked");

    // 4. Log in to NEW synthetic management session
    await act(async () => {
      await hook.result.current.unlock("synthetic-test-key");
    });
    expect(hook.result.current.session).toBe("open");

    // 5. In new session, start new-session PUT with "auto"
    let newSavePromise: Promise<unknown>;
    act(() => {
      newSavePromise = hook.result.current.saveGatewaySettings({ responses_mode: "auto" });
    });
    expect(putCount).toBe(2);
    expect(hook.result.current.sending).toBe(true);

    // 6. Old PUT resolves with 401
    let oldError: Error | null = null;
    await act(async () => {
      oldPutResolve!(json({ error: "unauthorized" }, 401));
      try {
        await oldSavePromise;
      } catch (err) {
        oldError = err as Error;
      }
    });

    // Caller of old PUT receives rejection
    expect(oldError).toBeInstanceOf(Error);
    // Crucial: New session must NOT be locked by old PUT's 401!
    expect(hook.result.current.session).toBe("open");
    // Crucial: New session's save guard / sending must still be owned by new session!
    expect(hook.result.current.sending).toBe(true);

    // 7. Resolve NEW PUT with "auto"
    await act(async () => {
      newPutResolve!(json({ responses_mode: "auto" }));
      await newSavePromise;
    });

    // New session successfully confirms "auto"
    expect(hook.result.current.session).toBe("open");
    expect(hook.result.current.sending).toBe(false);
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "auto" });
  });

  it("handles genuine reopened session: old in-flight PUT returning 200 does not overwrite new session mode or clear new session save guard", async () => {
    let oldPutResolve: ((res: Response) => void) | null = null;
    let newPutResolve: ((res: Response) => void) | null = null;
    let putCount = 0;

    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        if (method === "GET") return json({ responses_mode: "pass" });
        if (method === "PUT") {
          putCount++;
          if (putCount === 1) {
            return new Promise<Response>(resolve => {
              oldPutResolve = resolve;
            });
          }
          return new Promise<Response>(resolve => {
            newPutResolve = resolve;
          });
        }
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // 1. Initial session: load confirmed "pass"
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });

    // 2. Start old PUT in session 1
    let oldSavePromise: Promise<unknown>;
    act(() => {
      oldSavePromise = hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
    });
    expect(putCount).toBe(1);

    // 3. User logs out
    await act(async () => {
      await hook.result.current.logout();
    });
    expect(hook.result.current.session).toBe("locked");

    // 4. Log in to NEW synthetic management session
    await act(async () => {
      await hook.result.current.unlock("synthetic-test-key");
    });
    expect(hook.result.current.session).toBe("open");

    // 5. In new session, start new-session PUT with "auto"
    let newSavePromise: Promise<unknown>;
    act(() => {
      newSavePromise = hook.result.current.saveGatewaySettings({ responses_mode: "auto" });
    });
    expect(putCount).toBe(2);
    expect(hook.result.current.sending).toBe(true);

    // 6. Old PUT resolves with 200 { responses_mode: "adapt" }
    let oldError: Error | null = null;
    await act(async () => {
      oldPutResolve!(json({ responses_mode: "adapt" }));
      try {
        await oldSavePromise;
      } catch (err) {
        oldError = err as Error;
      }
    });

    // Old PUT was superseded by session change, so caller observes rejection
    expect(oldError).toBeInstanceOf(Error);
    expect((oldError as unknown as Error).message).toMatch(/保存操作已被取消或被新会话覆盖|已取消/);
    // Crucial: Old 200 must NOT overwrite gatewaySettings in the new session!
    expect(hook.result.current.gatewaySettings).toBeNull();
    // Crucial: New session's save guard / sending must still be active!
    expect(hook.result.current.sending).toBe(true);

    // 7. Resolve NEW PUT with 200 { responses_mode: "auto" }
    await act(async () => {
      newPutResolve!(json({ responses_mode: "auto" }));
      await newSavePromise;
    });

    // Confirmed state is "auto", owned by new session
    expect(hook.result.current.session).toBe("open");
    expect(hook.result.current.sending).toBe(false);
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "auto" });
  });

  it("rejects invalid PUT mode or empty DTO without updating confirmed state", async () => {
    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        if (method === "GET") return json({ responses_mode: "pass" });
        if (method === "PUT") {
          // Server returns invalid / malformed DTO without responses_mode
          return json({ invalid_field: 123 });
        }
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // Load initial confirmed "pass"
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });

    // Attempt save with invalid response from server
    let thrownError: Error | null = null;
    await act(async () => {
      try {
        await hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
      } catch (err) {
        thrownError = err as Error;
      }
    });

    expect(thrownError).toBeInstanceOf(Error);
    expect((thrownError as unknown as Error).message).toBe("服务端返回的网关设置格式无效");

    // Confirmed state MUST remain last confirmed "pass", never optimistic "adapt" or corrupt
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });
  });

  it("rejects hook save when session is locked without sending PUT request", async () => {
    const hook = await mount();

    // Lock session
    await act(async () => {
      await hook.result.current.logout();
    });
    expect(hook.result.current.session).toBe("locked");

    let lockedError: Error | null = null;
    await act(async () => {
      try {
        await hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
      } catch (err) {
        lockedError = err as Error;
      }
    });

    expect(lockedError).toBeInstanceOf(Error);
    expect((lockedError as unknown as Error).message).toBe("会话已锁定或未登录");

    // No PUT request should have been dispatched
    const putCalls = fetchMock.mock.calls.filter(
      call => String(call[0]) === "/api/gateway-settings" && call[1]?.method === "PUT"
    );
    expect(putCalls).toHaveLength(0);
  });

  it("skips GET started while PUT is pending to prevent overwrite/invalidated confirmed mutation, then allows fresh GET after settlement", async () => {
    let putResolve: ((res: Response) => void) | null = null;
    let getCallCount = 0;

    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings") {
        if (method === "GET") {
          getCallCount++;
          return json({ responses_mode: getCallCount === 1 ? "pass" : "adapt" });
        }
        if (method === "PUT") {
          return new Promise<Response>(resolve => {
            putResolve = resolve;
          });
        }
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // 1. Initial confirmed load
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "pass" });
    expect(getCallCount).toBe(1);

    // 2. Start PUT (slow in-flight)
    let savePromise: Promise<unknown>;
    act(() => {
      savePromise = hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
    });

    // 3. GET triggered while PUT is pending -> must be skipped because saving is in progress
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });
    // getCallCount did NOT increment because GET was safely skipped during save
    expect(getCallCount).toBe(1);

    // 4. Resolve PUT
    await act(async () => {
      putResolve!(json({ responses_mode: "adapt" }));
      await savePromise;
    });

    // Confirmed state is now "adapt"
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "adapt" });

    // 5. Fresh GET after settlement works normally
    await act(async () => {
      await hook.result.current.loadGatewaySettings();
    });
    expect(getCallCount).toBe(2);
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "adapt" });
  });

  it("dispatches exactly one PUT on double save, rejecting concurrent second save", async () => {
    let putResolve: ((res: Response) => void) | null = null;

    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings" && method === "PUT") {
        return new Promise<Response>(resolve => {
          putResolve = resolve;
        });
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // First save starts
    let firstSave: Promise<unknown>;
    act(() => {
      firstSave = hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
    });

    // Concurrent second save should reject immediately as busy
    let secondError: Error | null = null;
    await act(async () => {
      try {
        await hook.result.current.saveGatewaySettings({ responses_mode: "auto" });
      } catch (err) {
        secondError = err as Error;
      }
    });

    expect(secondError).toBeInstanceOf(Error);
    expect((secondError as unknown as Error).message).toBe("已有操作正在处理中");

    // Resolve first save
    await act(async () => {
      putResolve!(json({ responses_mode: "adapt" }));
      await firstSave;
    });

    // Exactly 1 PUT was sent
    const putCalls = fetchMock.mock.calls.filter(
      call => String(call[0]) === "/api/gateway-settings" && call[1]?.method === "PUT"
    );
    expect(putCalls).toHaveLength(1);
    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "adapt" });
  });

  it("releases saving guard on failed save and allows subsequent retry to succeed", async () => {
    let failFirst = true;

    fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      const method = init?.method || "GET";
      if (path === "/api/admin/session") return json(null, 204);
      if (path === "/api/accounts") return json({ accounts: [], route_account_id: null });
      if (path === "/api/checkin") {
        return json({
          settings: { enabled: false, time: "09:00", interval_minutes: 30, timezone: "Asia/Shanghai", reset_time: "08:00" },
          cycle_date: "2026-10-01", next_run_at: null, schedule: [],
        });
      }
      if (path === "/api/log-settings") return json({ level: "info", system_retention_days: 7, request_retention_days: 7 });
      if (path.includes("logs")) return json({ items: [], dropped_count: 0 });
      if (path === "/api/gateway-settings" && method === "PUT") {
        if (failFirst) {
          failFirst = false;
          return json({ error: "internal server error" }, 500);
        }
        return json({ responses_mode: "adapt" });
      }
      throw new Error(`Unexpected: ${path}`);
    });

    const hook = await mount();

    // First save fails
    let firstError: Error | null = null;
    await act(async () => {
      try {
        await hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
      } catch (err) {
        firstError = err as Error;
      }
    });

    expect(firstError).not.toBeNull();

    // Saving guard is released, allowing subsequent retry
    await act(async () => {
      const result = await hook.result.current.saveGatewaySettings({ responses_mode: "adapt" });
      expect(result).toEqual({ responses_mode: "adapt" });
    });

    expect(hook.result.current.gatewaySettings).toEqual({ responses_mode: "adapt" });
  });
});
