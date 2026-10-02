import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, fetchSystemLogs } from "./api";
import type { LoginDiagnostics, SystemLogsResponse } from "./api";

const diagnostics: LoginDiagnostics = {
  version: 1,
  phase: "profile_wait",
  page: "login",
  login_requested: true,
  login_status: 200,
  login_json: true,
  login_success: true,
  self_requested: true,
  self_status: null,
  self_json: false,
  self_success: null,
  self_id_type: "absent",
  self_id_valid: false,
  self_user_header_present: false,
  user_state_ready: false,
  pending_login: false,
  failure_request: "self",
  exception: "timeout",
};
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { "Content-Type": "application/json" },
});

afterEach(() => { vi.unstubAllGlobals(); });

describe("system-log API (mock HTTP only)", () => {
  it.each(["legacy", null, "notice_close", "notice_wait_hidden", "submit_trial", "page_recheck", "submit_click"] as const)("preserves structured diagnostics and optional action metadata (%s)", async action => {
    const metadata: LoginDiagnostics = action === "legacy" ? diagnostics : {
      ...diagnostics,
      action,
      action_timeout_ms: action === null ? null : 120000,
      action_elapsed_ms: action === null ? null : 0,
    };
    const operationId = "00000000-0000-4000-8000-000000000001";
    const page: SystemLogsResponse = {
      items: [{
        id: "00000000-0000-4000-8000-000000000002",
        timestamp: "2026-10-01T12:00:00Z",
        level: "debug", event: "helper_result", operation_id: operationId,
        account_id: null, stage: "helper_result", elapsed_ms: 28000,
        http_status: null, reason: null, diagnostics: metadata,
      }],
      dropped_count: 0,
    };
    const fetchMock = vi.fn().mockResolvedValue(json(page));
    vi.stubGlobal("fetch", fetchMock);
    const controller = new AbortController();
    const result = await fetchSystemLogs({ operation_id: operationId }, controller.signal);
    expect(result).toEqual(page);
    expect(result.items[0].diagnostics).toEqual(metadata);
    if (action === "legacy") {
      expect(result.items[0].diagnostics).not.toHaveProperty("action");
      expect(result.items[0].diagnostics).not.toHaveProperty("action_timeout_ms");
      expect(result.items[0].diagnostics).not.toHaveProperty("action_elapsed_ms");
    } else {
      expect(result.items[0].diagnostics).toHaveProperty("action", action);
      expect(result.items[0].diagnostics).toHaveProperty("action_timeout_ms", metadata.action_timeout_ms);
      expect(result.items[0].diagnostics).toHaveProperty("action_elapsed_ms", metadata.action_elapsed_ms);
    }
    expect(fetchMock).toHaveBeenCalledExactlyOnceWith(`/api/system-logs?operation_id=${operationId}`, {
      signal: controller.signal, credentials: "same-origin", cache: "no-store",
    });
  });

  it("preserves a successful empty response", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(json({ items: [], dropped_count: 0 })));
    await expect(fetchSystemLogs()).resolves.toEqual({ items: [], dropped_count: 0 });
  });

  it("propagates HTTP 503 instead of reporting an empty log", async () => {
    const detail = { code: "logging_unavailable", message: "Logging unavailable." };
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(json({ error: detail }, 503)));
    await expect(fetchSystemLogs()).rejects.toMatchObject({ status: 503, detail });
  });

  it("propagates network failure unchanged", async () => {
    const failure = new TypeError("Local mock network failure");
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(failure));
    await expect(fetchSystemLogs()).rejects.toBe(failure);
  });

  it("keeps 401 authentication errors unchanged", async () => {
    vi.stubGlobal("fetch", vi.fn().mockImplementation(async () => json(null, 401)));
    await expect(fetchSystemLogs()).rejects.toBeInstanceOf(ApiError);
    await expect(fetchSystemLogs()).rejects.toMatchObject({ status: 401, detail: { code: "unauthorized" } });
  });

  it("does not turn invalid successful JSON into empty logs", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("not JSON", { status: 200 })));
    await expect(fetchSystemLogs()).rejects.toBeInstanceOf(SyntaxError);
  });
});
