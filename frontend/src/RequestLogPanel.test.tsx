import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { RequestLogPanel, formatLogTime } from "./RequestLogPanel";
import type { RequestLogEntry } from "./accountsTypes";

function makeLog(partial: Partial<RequestLogEntry>): RequestLogEntry {
  return {
    id: "log-1",
    timestamp: "2026-10-01T02:00:00Z",
    accountId: "acc-1",
    accountName: "user-one",
    httpStatus: 200,
    errorBody: null,
    truncated: false,
    ...partial,
  };
}

const baseLogs: RequestLogEntry[] = [
  makeLog({ id: "log-1", httpStatus: 200, errorBody: null }),
  makeLog({
    id: "log-2",
    timestamp: "2026-10-01T02:05:00Z",
    accountId: "acc-2",
    accountName: "user-two",
    httpStatus: 429,
    errorBody: '{"error":{"message":"rate limited, please retry after 30s"}}',
  }),
  makeLog({
    id: "log-3",
    timestamp: "2026-10-01T02:10:00Z",
    accountId: "acc-1",
    accountName: "user-one",
    httpStatus: null,
    errorBody: null,
  }),
  makeLog({
    id: "log-4",
    timestamp: "2026-10-01T02:15:00Z",
    accountId: "acc-2",
    accountName: "user-two",
    httpStatus: 502,
    errorBody: "<html>upstream bad gateway page</html>",
    truncated: true,
  }),
];

describe("formatLogTime", () => {
  it("formats ISO timestamps in Asia/Shanghai regardless of local timezone", () => {
    expect(formatLogTime("2026-10-01T02:00:00Z")).toMatch(/10\/01 10:00:00|10-01 10:00:00|10\/1 10:00:00/);
    // +00:00 offset form also accepted
    expect(formatLogTime("2026-10-01T02:00:00+00:00")).toMatch(/10:00:00/);
  });
});

describe("RequestLogPanel", () => {
  it("renders columns: time, account, HTTP status only", () => {
    render(<RequestLogPanel logs={[baseLogs[0]]} />);
    expect(screen.getByRole("heading", { name: "网关请求日志" })).toBeInTheDocument();
    expect(screen.getByText("user-one")).toBeInTheDocument();
    expect(screen.getByText("200")).toBeInTheDocument();
    // 200 rows show no detail button
    expect(screen.queryByRole("button", { name: "查看错误原文" })).not.toBeInTheDocument();
  });

  it("200 entries never expose a body or detail button even if server sent one", () => {
    render(<RequestLogPanel logs={[makeLog({ httpStatus: 200, errorBody: "should-not-show" })]} />);
    expect(screen.queryByText("should-not-show")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "查看错误原文" })).not.toBeInTheDocument();
  });

  it("null status renders 未收到响应, not a fake 200", () => {
    render(<RequestLogPanel logs={[makeLog({ httpStatus: null })]} />);
    expect(screen.getByText("未收到响应")).toBeInTheDocument();
    expect(screen.queryByText("200")).not.toBeInTheDocument();
  });

  it("error entries expand raw upstream body via React text nodes (no dangerouslySetInnerHTML)", () => {
    render(<RequestLogPanel logs={[baseLogs[1]]} />);

    const toggle = screen.getByRole("button", { name: "查看错误原文" });
    fireEvent.click(toggle);

    const region = screen.getByRole("region", { name: /错误原文/ });
    // Rendered as text content inside <pre>, so HTML/script tags would never execute
    const pre = region.querySelector("pre.log-error-body");
    expect(pre).not.toBeNull();
    expect(pre?.textContent).toContain("rate limited");
  });

  it("shows truncation notice for oversized bodies", () => {
    render(<RequestLogPanel logs={[baseLogs[3]]} />);
    fireEvent.click(screen.getByRole("button", { name: "查看错误原文" }));
    expect(screen.getByText(/内容过长，已截断/)).toBeInTheDocument();
    expect(screen.getByText(/upstream bad gateway page/)).toBeInTheDocument();
  });

  it("filter tabs narrow by status class and show empty state", () => {
    render(<RequestLogPanel logs={[baseLogs[0], baseLogs[1]]} />);
    expect(screen.getByText(/共 2 条记录/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "异常 (4xx/5xx)" }));
    expect(screen.getByText(/显示 1 条/)).toBeInTheDocument();
    expect(screen.getByText("429")).toBeInTheDocument();
    expect(screen.queryByText("200")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "成功 (2xx)" }));
    expect(screen.getByText("200")).toBeInTheDocument();
    expect(screen.queryByText("429")).not.toBeInTheDocument();
  });

  it("loading prop disables refresh and relabels button (parent-controlled)", () => {
    const onRefresh = vi.fn();
    const { rerender } = render(<RequestLogPanel logs={[]} onRefresh={onRefresh} loading={false} />);

    expect(screen.getByText("暂无网关请求日志记录。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "刷新日志" }));
    expect(onRefresh).toHaveBeenCalledTimes(1);

    // Parent flips loading=true; button relabels and disables
    rerender(<RequestLogPanel logs={[]} onRefresh={onRefresh} loading={true} />);
    const refreshBtn = screen.getByRole("button", { name: "正在刷新…" });
    expect(refreshBtn).toBeDisabled();
  });

  it("panel-level error notice renders safely", () => {
    render(<RequestLogPanel logs={[]} error="日志服务暂时不可用" />);
    expect(screen.getByRole("alert")).toHaveTextContent("日志服务暂时不可用");
  });
});
