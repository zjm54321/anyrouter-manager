import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AddAccountModal } from "./AddAccountModal";
import type { CandidateDTO, OperationDTO } from "./api";

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const mockCandidate: CandidateDTO = {
  id: "cand-1",
  username: "test-candidate",
  upstream_user_id: "upstream-1",
  balance: {
    quota_raw: "500000",
    used_quota_raw: "0",
    fetched_at: "2026-10-01T12:00:00Z",
  },
  keys: [
    { id: "key-1", name: "Default Key", masked: "sk-proj-****1234", enabled: true },
  ],
  expires_at: "2099-01-01T00:00:00Z",
};

describe("AddAccountModal", () => {
  it("clears both username and password inputs when closed and reopened (modal_inputs_empty_on_reopen)", () => {
    const onClose = vi.fn();
    const { rerender } = render(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={vi.fn()}
        candidate={null}
        onSaveCandidate={vi.fn()}
        operation={null}
      />
    );

    const userInput = screen.getByLabelText("用户名");
    const passInput = screen.getByLabelText("密码");

    fireEvent.change(userInput, { target: { value: "my-user@example.com" } });
    fireEvent.change(passInput, { target: { value: "secret-123456" } });

    expect(userInput).toHaveValue("my-user@example.com");
    expect(passInput).toHaveValue("secret-123456");

    // Close via close button
    fireEvent.click(screen.getByRole("button", { name: "关闭对话框" }));
    expect(onClose).toHaveBeenCalled();

    // Parent sets isOpen to false
    rerender(
      <AddAccountModal
        isOpen={false}
        onClose={onClose}
        onLogin={vi.fn()}
        candidate={null}
        onSaveCandidate={vi.fn()}
        operation={null}
      />
    );

    // Parent reopens modal (isOpen=true)
    rerender(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={vi.fn()}
        candidate={null}
        onSaveCandidate={vi.fn()}
        operation={null}
      />
    );

    // Both username and password must be completely empty!
    expect(screen.getByLabelText("用户名")).toHaveValue("");
    expect(screen.getByLabelText("密码")).toHaveValue("");
  });

  it("clears inputs on Escape key and backdrop click", () => {
    const onClose = vi.fn();
    render(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={vi.fn()}
        candidate={null}
        onSaveCandidate={vi.fn()}
        operation={null}
      />
    );

    const userInput = screen.getByLabelText("用户名");
    const passInput = screen.getByLabelText("密码");
    fireEvent.change(userInput, { target: { value: "escape-user" } });
    fireEvent.change(passInput, { target: { value: "escape-pass" } });

    // Press Escape
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);

    // Click backdrop
    const backdrop = document.querySelector(".modal-backdrop");
    expect(backdrop).toBeTruthy();
    fireEvent.click(backdrop!);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("does not resurrect state or leak late error if modal was closed during pending login", async () => {
    const loginTask = deferred();
    const onClose = vi.fn();
    const onLogin = vi.fn().mockImplementation(() => loginTask.promise);

    const { rerender } = render(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={onLogin}
        candidate={null}
        onSaveCandidate={vi.fn()}
        operation={null}
      />
    );

    const userInput = screen.getByLabelText("用户名");
    const passInput = screen.getByLabelText("密码");
    fireEvent.change(userInput, { target: { value: "late-user" } });
    fireEvent.change(passInput, { target: { value: "late-pass" } });

    // Submit credentials -> password cleared immediately
    fireEvent.click(screen.getByRole("button", { name: "登录" }));
    expect(passInput).toHaveValue("");
    expect(onLogin).toHaveBeenCalledWith({ username: "late-user", password: "late-pass" });

    // User closes modal before loginTask resolves
    fireEvent.click(screen.getByRole("button", { name: "关闭对话框" }));
    expect(onClose).toHaveBeenCalled();

    // Reopen modal cleanly
    rerender(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={onLogin}
        candidate={null}
        onSaveCandidate={vi.fn()}
        operation={null}
      />
    );

    // Now late login rejection returns
    await loginTask.reject(new Error("网络超时"));

    // The new modal must NOT be polluted with the old late error!
    expect(screen.queryByText("网络超时")).not.toBeInTheDocument();
    expect(screen.getByLabelText("用户名")).toHaveValue("");
    expect(screen.getByLabelText("密码")).toHaveValue("");
  });

  it("distinguishes save 202 acceptance and operation failure vs terminal success", async () => {
    const onClose = vi.fn();
    const onSaveCandidate = vi.fn().mockResolvedValue(undefined);

    const { rerender } = render(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={vi.fn()}
        candidate={mockCandidate}
        onSaveCandidate={onSaveCandidate}
        operation={null}
      />
    );

    // Displays candidate username and keys
    expect(screen.getByText("test-candidate")).toBeInTheDocument();
    expect(screen.getByText("Default Key")).toBeInTheDocument();

    const saveBtn = screen.getByRole("button", { name: "保存账号" });
    fireEvent.click(saveBtn);
    expect(onSaveCandidate).toHaveBeenCalledWith("cand-1", "key-1");

    // Modal must NOT close immediately on 202 acceptance!
    expect(onClose).not.toHaveBeenCalled();

    // If operation fails, modal stays open and displays the error
    const failedOp: OperationDTO = {
      id: "op-save-1",
      kind: "save",
      status: "failed",
      phase: "saving_account",
      error: { code: "upstream_error", message: "上游保存失败：用户名冲突" },
      account_id: null,
    };

    rerender(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={vi.fn()}
        candidate={mockCandidate}
        onSaveCandidate={onSaveCandidate}
        operation={failedOp}
      />
    );

    expect(await screen.findByText("上游保存失败：用户名冲突")).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();

    // A finished failed attempt cannot later close the modal. Retry is a NEW ID.
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "保存账号" })));
    const successOp: OperationDTO = {
      id: "op-save-2",
      kind: "save",
      status: "succeeded",
      phase: "done",
      error: null,
      account_id: "acc-1",
    };

    rerender(
      <AddAccountModal
        isOpen={true}
        onClose={onClose}
        onLogin={vi.fn()}
        candidate={null}
        onSaveCandidate={onSaveCandidate}
        operation={successOp}
      />
    );

    // Modal closes automatically on terminal success!
    expect(onClose).toHaveBeenCalled();
  });

  it("does not close on missing candidate, old save success, or a different terminal ID", async () => {
    const onClose = vi.fn();
    const old: OperationDTO = { id: "old", kind: "save", status: "succeeded", phase: "done", error: null, account_id: "acc-1" };
    const props = { isOpen: true, onClose, onLogin: vi.fn(), onSaveCandidate: vi.fn().mockResolvedValue(undefined) };
    const { rerender } = render(<AddAccountModal {...props} candidate={mockCandidate} operation={old} />);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "保存账号" })));
    rerender(<AddAccountModal {...props} candidate={null} operation={old} />);
    expect(onClose).not.toHaveBeenCalled();
    const running = { ...old, id: "new", status: "running" as const };
    rerender(<AddAccountModal {...props} candidate={mockCandidate} operation={running} />);
    rerender(<AddAccountModal {...props} candidate={null} operation={{ ...old, id: "unrelated" }} />);
    expect(onClose).not.toHaveBeenCalled();
    rerender(<AddAccountModal {...props} candidate={mockCandidate} operation={{ ...old, id: "new" }} />);
    expect(onClose).not.toHaveBeenCalled(); // candidate must have been consumed too
    rerender(<AddAccountModal {...props} candidate={null} operation={{ ...old, id: "new" }} />);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("invalidates an old pending save on candidate replacement and close/reopen", async () => {
    const task = deferred();
    const onClose = vi.fn();
    const props = { isOpen: true, onClose, onLogin: vi.fn(), onSaveCandidate: () => task.promise };
    const { rerender } = render(<AddAccountModal {...props} candidate={mockCandidate} operation={null} />);
    fireEvent.click(screen.getByRole("button", { name: "保存账号" }));
    const next = { ...mockCandidate, id: "cand-2", username: "next-fixture" };
    rerender(<AddAccountModal {...props} candidate={next} operation={null} />);
    await act(async () => task.reject(new Error("old save error")));
    expect(screen.queryByText("old save error")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存账号" })).toBeEnabled();
    rerender(<AddAccountModal {...props} isOpen={false} candidate={next} operation={null} />);
    rerender(<AddAccountModal {...props} candidate={next} operation={{ id: "old", kind: "save", status: "succeeded", phase: "done", error: null, account_id: "acc-1" }} />);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText("next-fixture")).toBeInTheDocument();
  });

  it("retains key choice across polling snapshots and awaits acceptance AND terminal state", async () => {
    const task = deferred();
    const onClose = vi.fn();
    const save = vi.fn(() => task.promise);
    const props = { isOpen: true, onClose, onLogin: vi.fn(), onSaveCandidate: save };
    const { rerender } = render(<AddAccountModal {...props} candidate={mockCandidate} operation={null} />);
    fireEvent.click(screen.getByRole("radio", { name: /不选择密钥/ }));
    rerender(<AddAccountModal {...props} candidate={{ ...mockCandidate }} operation={null} />);
    fireEvent.click(screen.getByRole("button", { name: "保存账号" }));
    expect(save).toHaveBeenCalledWith(mockCandidate.id, null);
    rerender(<AddAccountModal {...props} candidate={null} operation={{ id: "new", kind: "save", status: "succeeded", phase: "done", error: null, account_id: "acc-1" }} />);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => task.resolve());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("does not close a reopened modal on a previous save's late acceptance and success", async () => {
    const task = deferred();
    const onClose = vi.fn();
    const props = { isOpen: true, onClose, onLogin: vi.fn(), onSaveCandidate: () => task.promise };
    const view = render(<AddAccountModal {...props} candidate={mockCandidate} operation={null} />);
    fireEvent.click(screen.getByRole("button", { name: "保存账号" }));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    view.rerender(<AddAccountModal {...props} isOpen={false} candidate={mockCandidate} operation={null} />);
    view.rerender(<AddAccountModal {...props} candidate={{ ...mockCandidate, id: "different-candidate" }} operation={null} />);
    await act(async () => task.resolve());
    view.rerender(<AddAccountModal {...props} candidate={null} operation={{ id: "previous-save", kind: "save", status: "succeeded", phase: "done", error: null, account_id: "acc-1" }} />);
    expect(onClose).toHaveBeenCalledTimes(1); // only the explicit Escape
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByLabelText("用户名")).toHaveValue("");
  });

  it("does not bind a stale poll arriving before the new save is accepted", async () => {
    const task = deferred();
    const onClose = vi.fn();
    const props = { isOpen: true, onClose, onLogin: vi.fn(), onSaveCandidate: () => task.promise };
    const view = render(<AddAccountModal {...props} candidate={mockCandidate} operation={null} />);
    fireEvent.click(screen.getByRole("button", { name: "保存账号" }));
    const terminal: OperationDTO = { id: "stale-poll", kind: "save", status: "succeeded", phase: "done", error: null, account_id: "acc-1" };
    view.rerender(<AddAccountModal {...props} candidate={mockCandidate} operation={terminal} />);
    // The hook suppresses old IDs after HTTP202, then exposes its matching ID.
    view.rerender(<AddAccountModal {...props} candidate={mockCandidate} operation={{ ...terminal, id: "accepted-save", status: "running" }} />);
    await act(async () => task.resolve());
    expect(onClose).not.toHaveBeenCalled();
    view.rerender(<AddAccountModal {...props} candidate={null} operation={{ ...terminal, id: "accepted-save" }} />);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("ignores an old async login error delivered as an operation after close", async () => {
    const props = { isOpen: true, onClose: vi.fn(), onLogin: vi.fn().mockResolvedValue(undefined), onSaveCandidate: vi.fn(), candidate: null };
    const view = render(<AddAccountModal {...props} operation={null} />);
    fireEvent.change(screen.getByLabelText("用户名"), { target: { value: "fixture-user" } });
    fireEvent.change(screen.getByLabelText("密码"), { target: { value: "fixture-password" } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "登录" })));
    view.rerender(<AddAccountModal {...props} isOpen={false} operation={null} />);
    view.rerender(<AddAccountModal {...props} operation={{ id: "old-login", kind: "login", status: "failed", phase: "logging_in", account_id: null, error: { code: "upstream_timeout", message: "previous-error" } }} />);
    expect(screen.queryByText("previous-error")).not.toBeInTheDocument();
    expect(screen.getByLabelText("用户名")).toHaveValue("");
    expect(screen.getByLabelText("密码")).toHaveValue("");
  });

  it.each(["cancel", "escape", "backdrop", "prop", "unmount"])("clears both credential fields on %s", path => {
    const props = { isOpen: true, onClose: vi.fn(), onLogin: vi.fn(), onSaveCandidate: vi.fn(), candidate: null, operation: null };
    const view = render(<AddAccountModal {...props} />);
    fireEvent.change(screen.getByLabelText("用户名"), { target: { value: "fixture-user" } });
    fireEvent.change(screen.getByLabelText("密码"), { target: { value: "fixture-password" } });
    if (path === "cancel") fireEvent.click(screen.getByRole("button", { name: "取消" }));
    if (path === "escape") fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    if (path === "backdrop") fireEvent.click(screen.getByRole("dialog").parentElement!);
    if (path === "unmount") { view.unmount(); render(<AddAccountModal {...props} />); }
    else { view.rerender(<AddAccountModal {...props} isOpen={false} />); view.rerender(<AddAccountModal {...props} />); }
    expect(screen.getByLabelText("用户名")).toHaveValue("");
    expect(screen.getByLabelText("密码")).toHaveValue("");
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
  });
});
