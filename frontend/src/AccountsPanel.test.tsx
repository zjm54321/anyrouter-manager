import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AccountsPanel, RevealKeyModal, formatTimeInSlot } from "./AccountsPanel";
import type { GlobalCheckinConfig, ManagedAccount } from "./accountsTypes";

const keyA = { id: "key-a", name: "Primary Key", masked: "sk-ab…1234" };
const keyB = { id: "key-b", name: "Backup Key", masked: "sk-cd…5678" };

function makeAccount(partial: Partial<ManagedAccount>): ManagedAccount {
  return {
    id: "acc-1",
    username: "user-one",
    upstreamUserId: "101",
    balance: { quotaRaw: "500000", usedQuotaRaw: "12000", fetchedAt: "2026-10-01T02:00:00Z" },
    selectedKey: keyA,
    keys: [keyA],
    checkin: null,
    isRouted: false,
    ...partial,
  };
}

const defaultConfig: GlobalCheckinConfig = {
  enabled: false,
  startTime: "09:00",
  intervalMinutes: 30,
  timezone: "Asia/Shanghai",
  resetTime: "08:00",
};

describe("formatTimeInSlot", () => {
  it("computes fixed slots from add order, not finish+interval", () => {
    expect(formatTimeInSlot("09:00", 30, 0)).toBe("09:00");
    expect(formatTimeInSlot("09:00", 30, 1)).toBe("09:30");
    expect(formatTimeInSlot("09:00", 30, 2)).toBe("10:00");
    expect(formatTimeInSlot("09:00", 30, 3)).toBe("10:30");
  });

  it("wraps around midnight for late slots", () => {
    expect(formatTimeInSlot("23:45", 30, 1)).toBe("00:15");
  });
});

describe("RevealKeyModal lifecycle", () => {
  function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
  }
  function submit() {
    fireEvent.change(screen.getByLabelText("密钥"), { target: { value: "fixture-root" } });
    fireEvent.click(screen.getByRole("button", { name: "验证并显示" }));
    expect(screen.getByLabelText("密钥")).toHaveValue("");
  }

  it.each(["close", "switch", "unmount"])("discards late key and error after %s", async path => {
    const pending = deferred<string>();
    const reveal = vi.fn(() => pending.promise);
    const props = { accountId: "acc-1", accountUsername: "fixture-one", onClose: vi.fn(), onReveal: reveal };
    const view = render(<RevealKeyModal {...props} />);
    submit();
    if (path === "close") fireEvent.click(screen.getByRole("button", { name: "关闭对话框" }));
    if (path === "switch") view.rerender(<RevealKeyModal {...props} accountId="acc-2" accountUsername="fixture-two" />);
    if (path === "unmount") view.unmount();
    const signal = (reveal.mock.calls[0] as unknown as [string, string, AbortSignal])[2];
    expect(signal.aborted).toBe(true);
    await act(async () => pending.resolve("must-not-appear"));
    expect(screen.queryByDisplayValue("must-not-appear")).not.toBeInTheDocument();
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
  });

  it("does not overwrite a new account's result with an old rejection", async () => {
    const old = deferred<string>();
    const reveal = vi.fn().mockImplementationOnce(() => old.promise).mockResolvedValue("new-key");
    const props = { accountId: "acc-1", accountUsername: "fixture-one", onClose: vi.fn(), onReveal: reveal };
    const view = render(<RevealKeyModal {...props} />);
    submit();
    view.rerender(<RevealKeyModal {...props} accountId="acc-2" />);
    submit();
    await act(async () => {});
    expect(screen.getByLabelText(/当前密钥/)).toHaveValue("new-key");
    await act(async () => old.reject(new Error("stale-error")));
    expect(screen.queryByText("stale-error")).not.toBeInTheDocument();
    expect(screen.getByLabelText(/当前密钥/)).toHaveValue("new-key");
  });

  it("clears account-change timers and rejects clipboard feedback after close", async () => {
    vi.useFakeTimers();
    const timeout = vi.spyOn(window, "setTimeout");
    const interval = vi.spyOn(window, "setInterval");
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    const clearInterval = vi.spyOn(window, "clearInterval");
    const copied = deferred<void>();
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(() => copied.promise) } });
    const props = { accountId: "acc-1", accountUsername: "fixture", onClose: vi.fn(), onReveal: vi.fn().mockResolvedValue("transient-key") };
    try {
      const view = render(<RevealKeyModal {...props} />);
      submit();
      await act(async () => {});
      fireEvent.click(screen.getByRole("button", { name: "复制" }));
      fireEvent.click(screen.getByRole("button", { name: "关闭对话框" }));
      await act(async () => copied.resolve());
      expect(screen.queryByText("已复制到剪贴板")).not.toBeInTheDocument();
      const autoHide = () => timeout.mock.calls.map((call, index) => ({ delay: call[1], index }))
        .filter(call => call.delay === 30000).map(call => timeout.mock.results[call.index].value).pop();
      const countdown = () => interval.mock.calls.map((call, index) => ({ delay: call[1], index }))
        .filter(call => call.delay === 1000).map(call => interval.mock.results[call.index].value).pop();
      expect(clearTimeout).toHaveBeenCalledWith(autoHide());
      expect(clearInterval).toHaveBeenCalledWith(countdown());
      submit();
      await act(async () => {});
      const nextHide = autoHide(), nextCountdown = countdown();
      view.rerender(<RevealKeyModal {...props} accountId="acc-2" />);
      expect(screen.queryByDisplayValue("transient-key")).not.toBeInTheDocument();
      expect(screen.getByLabelText("密钥")).toHaveValue("");
      expect(clearTimeout).toHaveBeenCalledWith(nextHide);
      expect(clearInterval).toHaveBeenCalledWith(nextCountdown);
      view.unmount();
    } finally { vi.restoreAllMocks(); vi.useRealTimers(); }
  });
});

describe("AccountsPanel", () => {
  const baseProps = {
    accounts: [] as ManagedAccount[],
    routedAccountId: null as string | null,
    globalCheckinConfig: defaultConfig,
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders empty state when no accounts are bound", () => {
    render(<AccountsPanel {...baseProps} />);
    expect(screen.getByRole("heading", { name: "已绑定账号列表" })).toBeInTheDocument();
    expect(screen.getByText(/尚未添加任何账号/)).toBeInTheDocument();
    expect(screen.getByText(/尚未指定路由账号/)).toBeInTheDocument();
  });

  it("shows routed badge on the routed account and disables its route button", () => {
    const accounts = [
      makeAccount({ id: "acc-1", username: "routed-user" }),
      makeAccount({ id: "acc-2", username: "other-user" }),
    ];
    render(<AccountsPanel {...baseProps} accounts={accounts} routedAccountId="acc-1" />);

    const routedCard = screen.getByTestId("account-card-acc-1");
    expect(routedCard).toHaveTextContent("当前路由");

    const otherCard = screen.getByTestId("account-card-acc-2");
    expect(otherCard).not.toHaveTextContent("当前路由");

    // Click routed card to open drawer: button is disabled and shows "当前正在使用"
    fireEvent.click(routedCard);
    const routedBtn = screen.getByRole("button", { name: "当前正在使用" });
    expect(routedBtn).toBeDisabled();

    // Click other card to open drawer: button can be selected
    fireEvent.click(otherCard);
    const otherBtn = screen.getByRole("button", { name: "使用此账号" });
    expect(otherBtn).toBeEnabled();
  });

  it("disables route button with explanation when account has no key", () => {
    const accounts = [makeAccount({ id: "acc-1", selectedKey: null, keys: [] })];
    render(<AccountsPanel {...baseProps} accounts={accounts} />);

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    const btn = screen.getByRole("button", { name: "未选择密钥" });
    expect(btn).toBeDisabled();
    expect(screen.getByText(/此账号暂未关联有效密钥/)).toBeInTheDocument();
    // No auto key generation prompt
    expect(screen.queryByRole("button", { name: /生成密钥|创建密钥/ })).not.toBeInTheDocument();
  });

  it("dispatches route selection without switching automatically", async () => {
    const onSelectRoute = vi.fn();
    const accounts = [makeAccount({ id: "acc-1" })];
    render(
      <AccountsPanel {...baseProps} accounts={accounts} onSelectRoute={onSelectRoute} />
    );

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    fireEvent.click(screen.getByRole("button", { name: "使用此账号" }));
    await waitFor(() => expect(onSelectRoute).toHaveBeenCalledWith("acc-1"));
  });

  it("shows pending badge on pending route account and keeps old route badge", () => {
    const accounts = [
      makeAccount({ id: "acc-1", username: "old-route" }),
      makeAccount({ id: "acc-2", username: "new-route" }),
    ];
    render(
      <AccountsPanel
        {...baseProps}
        accounts={accounts}
        routedAccountId="acc-1"
        pendingRouteAccountId="acc-2"
      />
    );

    expect(screen.getByTestId("account-card-acc-1")).toHaveTextContent("当前路由");
    expect(screen.getByTestId("account-card-acc-2")).toHaveTextContent("切换中…");
    // Pending account button disabled while pending inside drawer
    fireEvent.click(screen.getByTestId("account-card-acc-2"));
    expect(screen.getByRole("button", { name: "正在切换路由…" })).toBeDisabled();
  });

  it("add account form clears password immediately at dispatch and does not persist", async () => {
    const onAddAccount = vi.fn().mockResolvedValue(undefined);
    render(<AccountsPanel {...baseProps} onAddAccount={onAddAccount} />);

    fireEvent.click(screen.getByRole("button", { name: "添加账号" }));

    const usernameInput = await screen.findByLabelText("用户名");
    const passwordInput = screen.getByLabelText("密码");

    fireEvent.change(usernameInput, { target: { value: "new-user@example.com" } });
    fireEvent.change(passwordInput, { target: { value: "secret-pass-1" } });

    const submitBtn = screen.getByRole("button", { name: "登录" });
    expect(submitBtn).toBeEnabled();
    fireEvent.click(submitBtn);

    // Password cleared at dispatch time (before promise resolves)
    expect(passwordInput).toHaveValue("");
    await waitFor(() =>
      expect(onAddAccount).toHaveBeenCalledWith({
        username: "new-user@example.com",
        password: "secret-pass-1",
      })
    );
  });

  it("cancel clears password and closes modal", async () => {
    render(<AccountsPanel {...baseProps} onAddAccount={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "添加账号" }));
    const usernameInput = await screen.findByLabelText("用户名");
    const passwordInput = screen.getByLabelText("密码");
    fireEvent.change(usernameInput, { target: { value: "should-vanish-user" } });
    fireEvent.change(passwordInput, { target: { value: "should-vanish" } });

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // Reopen: both username and password empty!
    fireEvent.click(screen.getByRole("button", { name: "添加账号" }));
    expect(await screen.findByLabelText("用户名")).toHaveValue("");
    expect(screen.getByLabelText("密码")).toHaveValue("");
  });

  it("manual checkin dispatches with confirmRetry: false for normal status", async () => {
    const onManualCheckin = vi.fn();
    const accounts = [makeAccount({ id: "acc-1" })];
    render(
      <AccountsPanel {...baseProps} accounts={accounts} onManualCheckin={onManualCheckin} />
    );

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    fireEvent.click(screen.getByRole("button", { name: "手动签到" }));
    await waitFor(() =>
      expect(onManualCheckin).toHaveBeenCalledWith("acc-1", { confirmRetry: false })
    );
  });

  it("unknown checkin status requires explicit confirm dialog before retry", async () => {
    const onManualCheckin = vi.fn();
    const accounts = [
      makeAccount({
        id: "acc-1",
        checkin: {
          status: "unknown",
          date: "2026-10-01",
          code: "upstream_5xx",
        },
      }),
    ];
    render(
      <AccountsPanel {...baseProps} accounts={accounts} onManualCheckin={onManualCheckin} />
    );

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    fireEvent.click(screen.getByRole("button", { name: "重试签到" }));

    // Dialog appears; onManualCheckin NOT yet called
    const dialog = screen.getByRole("dialog", { name: "确认重试签到" });
    expect(dialog).toBeInTheDocument();
    expect(onManualCheckin).not.toHaveBeenCalled();

    // Cancel does not dispatch
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onManualCheckin).not.toHaveBeenCalled();

    // Reopen and confirm dispatches with confirmRetry: true
    fireEvent.click(screen.getByRole("button", { name: "重试签到" }));
    fireEvent.click(await screen.findByRole("button", { name: "确认继续重试" }));
    await waitFor(() =>
      expect(onManualCheckin).toHaveBeenCalledWith("acc-1", { confirmRetry: true })
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("done checkin disables manual button", () => {
    const accounts = [
      makeAccount({
        id: "acc-1",
        checkin: { status: "success", date: "2026-10-01" },
      }),
    ];
    render(<AccountsPanel {...baseProps} accounts={accounts} />);
    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    const btn = screen.getByRole("button", { name: "今日已完成" });
    expect(btn).toBeDisabled();
  });

  it("global checkin settings: interval default 30, editable, saves dirty config only", async () => {
    const onUpdateGlobalCheckin = vi.fn().mockResolvedValue(undefined);
    render(
      <AccountsPanel
        {...baseProps}
        globalCheckinConfig={defaultConfig}
        onUpdateGlobalCheckin={onUpdateGlobalCheckin}
      />
    );

    const intervalInput = screen.getByLabelText("账号签到间隔（分钟）");
    expect(intervalInput).toHaveValue(30);

    // Save disabled until dirty
    const saveBtn = screen.getByRole("button", { name: "保存全局签到配置" });
    expect(saveBtn).toBeDisabled();

    fireEvent.change(intervalInput, { target: { value: "45" } });
    expect(saveBtn).toBeEnabled();

    fireEvent.click(saveBtn);
    await waitFor(() =>
      expect(onUpdateGlobalCheckin).toHaveBeenCalledWith({
        enabled: false,
        startTime: "09:00",
        intervalMinutes: 45,
      })
    );
    expect(await screen.findByText("配置已保存")).toBeInTheDocument();
  });

  it("calculates slot examples per add order via formatTimeInSlot", () => {
    expect(formatTimeInSlot("09:00", 30, 0)).toBe("09:00");
    expect(formatTimeInSlot("09:00", 30, 1)).toBe("09:30");
    expect(formatTimeInSlot("09:00", 30, 2)).toBe("10:00");
  });

  it("key reveal requires root re-auth and auto-clears value at 30s", async () => {
    const onRevealKey = vi.fn().mockResolvedValue("sk-revealed-full-key");
    const accounts = [makeAccount({ id: "acc-1" })];
    render(<AccountsPanel {...baseProps} accounts={accounts} onRevealKey={onRevealKey} />);

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    fireEvent.click(screen.getByRole("button", { name: "查看密钥" }));

    const rootInput = screen.getByLabelText("密钥");
    fireEvent.change(rootInput, { target: { value: "root-secret" } });

    vi.useFakeTimers();
    try {
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "验证并显示" }));
      });
      // Root input cleared at dispatch
      expect(rootInput).toHaveValue("");

      const keyInput = screen.getByLabelText(/当前密钥/);
      expect(keyInput).toHaveValue("sk-revealed-full-key");
      expect(onRevealKey).toHaveBeenCalledWith("acc-1", "root-secret", expect.anything());

      // Auto hide at 30s
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30000);
      });
      expect(screen.queryByLabelText(/当前密钥/)).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("key select appears for multi-key account and dispatches selection", async () => {
    const onSelectKey = vi.fn();
    const accounts = [makeAccount({ id: "acc-1", keys: [keyA, keyB] })];
    render(
      <AccountsPanel {...baseProps} accounts={accounts} onSelectKey={onSelectKey} />
    );

    fireEvent.click(screen.getByTestId("account-card-acc-1"));
    const select = screen.getByLabelText(/切换密钥/);
    fireEvent.change(select, { target: { value: "key-b" } });
    await waitFor(() => expect(onSelectKey).toHaveBeenCalledWith("acc-1", "key-b"));
  });

  it("refresh balance dispatches per account", async () => {
    const onRefreshBalance = vi.fn();
    const accounts = [makeAccount({ id: "acc-1" })];
    render(
      <AccountsPanel {...baseProps} accounts={accounts} onRefreshBalance={onRefreshBalance} />
    );

    fireEvent.click(screen.getByRole("button", { name: "刷新余额" }));
    await waitFor(() => expect(onRefreshBalance).toHaveBeenCalledWith("acc-1"));
  });
});
