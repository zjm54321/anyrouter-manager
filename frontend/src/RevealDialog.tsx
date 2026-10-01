import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { ApiError, failureText, revealAccountKey } from "./api";
import type { AccountDTO, RevealedKeyResponse } from "./api";
import type { ManagedAccount } from "./accountsTypes";

export interface RevealDialogAccount {
  id: string;
  revision?: string;
  username?: string;
  selected_key?: { id: string; name?: string; masked?: string } | null;
  selectedKey?: { id: string; name?: string; masked?: string } | null;
}

export interface RevealDialogProps {
  account?: RevealDialogAccount | AccountDTO | ManagedAccount | null;
  active?: RevealDialogAccount | AccountDTO | ManagedAccount | null; // backward compatibility
  onClose: () => void;
  onReveal?: (accountId: string, rootKey: string, signal?: AbortSignal) => Promise<string>;
}

export function RevealDialog({ account: propAccount, active, onClose, onReveal }: RevealDialogProps) {
  const account = propAccount === undefined ? active : propAccount;
  const [root, setRoot] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [copied, setCopied] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const controller = useRef<AbortController | null>(null);
  const generation = useRef(0);
  const mounted = useRef(false);
  const identity = useRef({ id: account?.id, revision: account?.revision });
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  function current(epoch: number, expected: typeof identity.current) {
    return mounted.current && generation.current === epoch && identity.current === expected;
  }

  function invalidate() {
    generation.current++;
    controller.current?.abort();
    controller.current = null;
    if (hideTimer.current !== null) clearTimeout(hideTimer.current);
    if (copyTimer.current !== null) clearTimeout(copyTimer.current);
    hideTimer.current = copyTimer.current = null;
  }

  function clearSensitiveState() {
    invalidate();
    setRoot("");
    setKey("");
    setMessage("");
    setBusy(false);
    setCopied(false);
  }

  useLayoutEffect(() => {
    mounted.current = true;
    const previousFocus = document.activeElement as HTMLElement | null;
    panel.current?.querySelector<HTMLInputElement>("input")?.focus();
    return () => {
      mounted.current = false;
      invalidate();
      previousFocus?.focus();
    };
  }, []);

  // Compare with the previous COMMITTED identity, not this render's props.
  // Layout cleanup removes any displayed secret before the new identity paints.
  useLayoutEffect(() => {
    if (account?.revision !== identity.current.revision || account?.id !== identity.current.id) {
      clearSensitiveState();
      identity.current = { id: account?.id, revision: account?.revision };
    }
  }, [account?.revision, account?.id]);

  useEffect(() => {
    if (!key) return;
    const epoch = generation.current;
    const expected = identity.current;
    panel.current?.querySelector<HTMLTextAreaElement>("textarea")?.focus();
    const timer = setTimeout(() => {
      if (!current(epoch, expected)) return;
      clearSensitiveState();
      setMessage("已自动隐藏密钥。如需查看，请重新验证");
    }, 30_000);
    hideTimer.current = timer;
    return () => {
      clearTimeout(timer);
      if (hideTimer.current === timer) hideTimer.current = null;
    };
  }, [key]);

  useEffect(() => {
    if (!copied) return;
    const epoch = generation.current;
    const expected = identity.current;
    const timer = setTimeout(() => {
      if (current(epoch, expected)) setCopied(false);
    }, 2000);
    copyTimer.current = timer;
    return () => {
      clearTimeout(timer);
      if (copyTimer.current === timer) copyTimer.current = null;
    };
  }, [copied]);

  function close() {
    clearSensitiveState();
    onClose();
  }

  async function reveal(event: FormEvent) {
    event.preventDefault();
    if (!account || !root.trim() || busy) return;
    controller.current?.abort();
    const task = new AbortController();
    controller.current = task;
    const epoch = ++generation.current;
    const expected = identity.current;
    const submittedRoot = root.trim();
    setRoot("");
    setBusy(true);
    setMessage("");

    try {
      let revealedKeyStr: string;
      if (onReveal) {
        revealedKeyStr = await onReveal(account.id, submittedRoot, task.signal);
      } else {
        const result: RevealedKeyResponse = await revealAccountKey(
          account.id,
          account.revision || "",
          submittedRoot,
          task.signal
        );
        if (!current(epoch, expected) || task.signal.aborted) return;
        if (result.account_id !== expected.id || result.revision !== expected.revision) {
          setMessage("账号版本已变化，已丢弃返回的密钥，请重新读取账号状态");
          return;
        }
        revealedKeyStr = result.key;
      }

      if (!current(epoch, expected) || task.signal.aborted) return;
      setKey(revealedKeyStr);
      setMessage("密钥将在 30 秒后自动隐藏");
    } catch (error) {
      if (!current(epoch, expected) || task.signal.aborted) return;
      setMessage(
        error instanceof ApiError && error.status === 401
          ? "本地 root 密钥或管理会话未通过验证，请重试"
          : error instanceof ApiError && error.detail.code === "stale_revision"
          ? "账号版本已变化，请关闭窗口后重新读取账号状态"
          : failureText(error)
      );
    } finally {
      if (current(epoch, expected) && !task.signal.aborted) setBusy(false);
    }
  }

  async function copy() {
    if (!key) return;
    const epoch = generation.current;
    const expected = identity.current;
    try {
      await navigator.clipboard.writeText(key);
      if (current(epoch, expected)) setCopied(true);
    } catch {
      if (current(epoch, expected)) setMessage("复制失败，请手动复制");
    }
  }

  return (
    <div className="modal-backdrop">
      <div
        className="card modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="reveal-title"
        ref={panel}
        onKeyDown={event => {
          if (event.key === "Escape") {
            event.preventDefault();
            close();
          }
          if (event.key !== "Tab") return;
          const elements = panel.current?.querySelectorAll<HTMLElement>(
            "button:not(:disabled), input:not(:disabled), textarea:not(:disabled), [tabindex='0']"
          );
          if (!elements?.length) return;
          const first = elements[0];
          const last = elements[elements.length - 1];
          if (
            event.shiftKey &&
            (document.activeElement === first || !panel.current?.contains(document.activeElement))
          ) {
            event.preventDefault();
            last.focus();
          } else if (
            !event.shiftKey &&
            (document.activeElement === last || !panel.current?.contains(document.activeElement))
          ) {
            event.preventDefault();
            first.focus();
          }
        }}
      >
        <div className="section-heading">
          <h2 id="reveal-title">查看密钥{account?.username ? ` · ${account.username}` : ""}</h2>
          <button type="button" onClick={close}>
            关闭
          </button>
        </div>
        <p className="secondary">
          输入本地 root 密钥以验证并显示。明文仅在本窗口短时保留，不会保存到浏览器存储。
        </p>
        {key ? (
          <div className="stack">
            <label htmlFor="revealed-key">当前密钥</label>
            <textarea
              id="revealed-key"
              readOnly
              value={key}
              rows={3}
              autoComplete="off"
              spellCheck={false}
            />
            <div className="actions">
              <button type="button" onClick={copy}>
                {copied ? "已复制" : "复制密钥"}
              </button>
              <button
                type="button"
                onClick={() => {
                  clearSensitiveState();
                  setMessage("已隐藏密钥");
                }}
              >
                隐藏 / 清除
              </button>
            </div>
          </div>
        ) : (
          <form onSubmit={reveal} className="stack">
            <label htmlFor="reveal-root">密钥</label>
            <input
              id="reveal-root"
              type="password"
              autoComplete="off"
              value={root}
              onChange={event => setRoot(event.target.value)}
              readOnly={busy}
              required
            />
            <button className="primary" disabled={!account || busy || !root.trim()}>
              {busy ? "正在验证…" : "验证并显示"}
            </button>
          </form>
        )}
        <p className="feedback" role="status">
          {message}
        </p>
      </div>
    </div>
  );
}
