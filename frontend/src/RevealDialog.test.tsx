import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "./api";
import { RevealDialog } from "./RevealDialog";
import type { RevealDialogAccount } from "./RevealDialog";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const accountA: RevealDialogAccount = { id: "fixture-a", revision: "rev-1", username: "Fixture A" };
const revision2 = { ...accountA, revision: "rev-2" };
const accountB = { ...accountA, id: "fixture-b", username: "Fixture B" };
const fakeRoot = "fake-root-sentinel";
const fakeKey = "fake-key-sentinel";

function submit() {
  const input = screen.getByLabelText("密钥");
  fireEvent.change(input, { target: { value: fakeRoot } });
  fireEvent.click(screen.getByRole("button", { name: "验证并显示" }));
  expect(input).toHaveValue("");
}

describe("RevealDialog identity isolation", () => {
  beforeEach(() => {
    Object.defineProperty(navigator, "clipboard", { configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) } });
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it.each([
    ["revision", revision2], ["account", accountB], ["removal", null],
  ] as const)("discards ignored-abort late success after %s changes on the same instance", async (_, next) => {
    const old = deferred<string>();
    const onReveal = vi.fn((_id: string, _root: string, _signal?: AbortSignal) => old.promise);
    const onClose = vi.fn();
    const view = render(<RevealDialog account={accountA} onClose={onClose} onReveal={onReveal} />);
    const panel = screen.getByRole("dialog");
    submit();
    const signal = onReveal.mock.calls[0][2]!;
    view.rerender(<RevealDialog account={next} onClose={onClose} onReveal={onReveal} />);
    expect(screen.getByRole("dialog")).toBe(panel); // no key/remount workaround
    expect(signal.aborted).toBe(true);
    expect(screen.getByLabelText("密钥")).toHaveValue("");
    expect(screen.queryByRole("button", { name: "正在验证…" })).not.toBeInTheDocument();
    await act(async () => old.resolve(fakeKey));
    expect(screen.queryByDisplayValue(fakeKey)).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
    expect(onClose).not.toHaveBeenCalled();
  });

  it.each(["success", "error"])("old %s/finally cannot clear the new request's busy state", async outcome => {
    const old = deferred<string>(), fresh = deferred<string>();
    const onReveal = vi.fn().mockImplementationOnce(() => old.promise).mockImplementationOnce(() => fresh.promise);
    const props = { onClose: vi.fn(), onReveal };
    const view = render(<RevealDialog {...props} account={accountA} />);
    submit();
    view.rerender(<RevealDialog {...props} account={revision2} />);
    submit();
    expect(onReveal).toHaveBeenCalledTimes(2);
    await act(async () => {
      if (outcome === "success") old.resolve("old-key-must-not-show");
      else old.reject(new Error("old-error-must-not-show"));
    });
    expect(screen.getByRole("button", { name: "正在验证…" })).toBeDisabled();
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
    expect(screen.queryByDisplayValue("old-key-must-not-show")).not.toBeInTheDocument();
    await act(async () => fresh.resolve(fakeKey));
    expect(screen.getByLabelText("当前密钥")).toHaveValue(fakeKey);
  });

  it.each([
    ["revision", revision2], ["account", accountB], ["removal", null],
  ] as const)("clears an already displayed key, copy feedback, timers and root on %s change", async (_, next) => {
    vi.useFakeTimers();
    const result = deferred<string>();
    const props = { onClose: vi.fn(), onReveal: vi.fn(() => result.promise) };
    const view = render(<RevealDialog {...props} account={accountA} />);
    submit();
    await act(async () => result.resolve(fakeKey));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "复制密钥" })));
    expect(screen.getByRole("button", { name: "已复制" })).toBeInTheDocument();
    view.rerender(<RevealDialog {...props} account={next} />);
    expect(screen.queryByLabelText("当前密钥")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
    expect(screen.getByLabelText("密钥")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("密钥"), { target: { value: fakeRoot } });
    view.rerender(<RevealDialog {...props} account={accountA} />);
    expect(screen.getByLabelText("密钥")).toHaveValue("");
    await act(async () => vi.advanceTimersByTime(30_000));
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
  });

  it("explicit account=null overrides the legacy active prop and restoration can verify again", async () => {
    const result = deferred<string>();
    const onReveal = vi.fn((_id: string, _root: string, _signal?: AbortSignal) => result.promise);
    const props = { onClose: vi.fn(), onReveal, active: accountA };
    const view = render(<RevealDialog {...props} />);
    fireEvent.change(screen.getByLabelText("密钥"), { target: { value: fakeRoot } });
    view.rerender(<RevealDialog {...props} account={null} />);
    expect(screen.getByLabelText("密钥")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("密钥"), { target: { value: fakeRoot } });
    expect(screen.getByRole("button", { name: "验证并显示" })).toBeDisabled();
    view.rerender(<RevealDialog {...props} account={accountB} />);
    expect(screen.getByLabelText("密钥")).toHaveValue("");
    submit();
    await act(async () => result.resolve(fakeKey));
    expect(onReveal).toHaveBeenCalledWith(accountB.id, fakeRoot, expect.any(AbortSignal));
    expect(screen.getByLabelText("当前密钥")).toHaveValue(fakeKey);
  });

  it.each(["success", "error"])("late clipboard %s does not update a new identity", async outcome => {
    const copied = deferred<void>();
    vi.mocked(navigator.clipboard.writeText).mockReturnValue(copied.promise);
    const result = deferred<string>();
    const props = { onClose: vi.fn(), onReveal: vi.fn(() => result.promise) };
    const view = render(<RevealDialog {...props} account={accountA} />);
    submit();
    await act(async () => result.resolve(fakeKey));
    fireEvent.click(screen.getByRole("button", { name: "复制密钥" }));
    view.rerender(<RevealDialog {...props} account={revision2} />);
    await act(async () => {
      if (outcome === "success") copied.resolve();
      else copied.reject(new Error("clipboard-fixture-error"));
    });
    expect(screen.queryByRole("button", { name: "已复制" })).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });

  it.each(["close", "unmount"])("aborts and discards a late promise on %s", async action => {
    const pending = deferred<string>();
    const onReveal = vi.fn((_id: string, _root: string, _signal?: AbortSignal) => pending.promise);
    const onClose = vi.fn();
    const view = render(<RevealDialog account={accountA} onClose={onClose} onReveal={onReveal} />);
    submit();
    if (action === "close") fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    else view.unmount();
    expect(onReveal.mock.calls[0][2]!.aborted).toBe(true);
    await act(async () => pending.resolve(fakeKey));
    expect(screen.queryByDisplayValue(fakeKey)).not.toBeInTheDocument();
    expect(onClose).toHaveBeenCalledTimes(action === "close" ? 1 : 0);
  });

  it("keeps 30s expiry, explicit copy, hide, focus trap and return focus in StrictMode", async () => {
    vi.useFakeTimers();
    const previous = document.createElement("button");
    document.body.append(previous);
    previous.focus();
    const result = deferred<string>();
    const view = render(<StrictMode><RevealDialog account={accountA} onClose={vi.fn()} onReveal={() => result.promise} /></StrictMode>);
    expect(screen.getByLabelText("密钥")).toHaveFocus();
    fireEvent.change(screen.getByLabelText("密钥"), { target: { value: fakeRoot } });
    screen.getByRole("button", { name: "关闭" }).focus();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Tab", shiftKey: true });
    expect(screen.getByRole("button", { name: "验证并显示" })).toHaveFocus();
    submit();
    await act(async () => result.resolve(fakeKey));
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTime(29_999));
    expect(screen.getByLabelText("当前密钥")).toHaveValue(fakeKey);
    await act(async () => vi.advanceTimersByTime(1));
    expect(screen.queryByLabelText("当前密钥")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("已自动隐藏密钥");
    submit();
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "隐藏 / 清除" }));
    expect(screen.queryByDisplayValue(fakeKey)).not.toBeInTheDocument();
    view.unmount();
    expect(previous).toHaveFocus();
    previous.remove();
  });

  it("keeps root 401 inline and clears the error on revision change", async () => {
    const pending = deferred<string>();
    const onClose = vi.fn();
    const props = { onClose, onReveal: () => pending.promise };
    const view = render(<RevealDialog {...props} account={accountA} />);
    submit();
    await act(async () => pending.reject(new ApiError(401, { code: "unauthorized", message: "fixture" })));
    expect(screen.getByRole("status")).toHaveTextContent("本地 root 密钥或管理会话未通过验证");
    expect(onClose).not.toHaveBeenCalled();
    view.rerender(<RevealDialog {...props} account={revision2} />);
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });
});
