import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { ApiError, failureText, request } from "./api";
import type { ActiveAccount, RevealedKey } from "./api";

export function RevealDialog({ active, onClose }: { active: ActiveAccount; onClose: () => void }) {
  const [root, setRoot] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [copied, setCopied] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const controller = useRef<AbortController | null>(null);
  const generation = useRef(0);

  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    panel.current?.querySelector<HTMLInputElement>("input")?.focus();
    return () => {
      generation.current++;
      controller.current?.abort();
      previousFocus?.focus();
    };
  }, []);

  useEffect(() => {
    if (!key) return;
    panel.current?.querySelector<HTMLTextAreaElement>("textarea")?.focus();
    const timer = window.setTimeout(() => {
      generation.current++;
      setKey("");
      setCopied(false);
      setMessage("已自动隐藏密钥。如需查看，请重新验证");
    }, 30_000);
    return () => window.clearTimeout(timer);
  }, [key]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  function close() {
    generation.current++;
    controller.current?.abort();
    setRoot("");
    setKey("");
    onClose();
  }

  async function reveal(event: FormEvent) {
    event.preventDefault();
    controller.current?.abort();
    const task = new AbortController();
    controller.current = task;
    const epoch = ++generation.current;
    setBusy(true);
    setMessage("");
    const pending = request<RevealedKey>("/api/account/key/reveal", {
      method: "POST", signal: task.signal,
      headers: { Authorization: `Bearer ${root}`, "Content-Type": "application/json" },
      body: JSON.stringify({ active_revision: active.revision }),
    });
    setRoot("");
    try {
      const result = await pending;
      if (epoch !== generation.current || task.signal.aborted) return;
      if (result.active_revision !== active.revision || result.key_id !== active.selected_key.id) {
        setMessage("账号版本已变化，已丢弃返回的密钥，请重新读取账号状态");
        return;
      }
      setKey(result.key);
      setMessage("密钥将在 30 秒后自动隐藏");
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      setMessage(error instanceof ApiError && error.status === 401
        ? "本地 root 密钥或管理会话未通过验证，请重试"
        : error instanceof ApiError && error.detail.code === "stale_revision"
          ? "账号版本已变化，请关闭窗口后重新读取账号状态"
          : failureText(error));
    } finally {
      if (epoch === generation.current) setBusy(false);
    }
  }

  async function copy() {
    const epoch = generation.current;
    try {
      await navigator.clipboard.writeText(key);
      if (epoch === generation.current) setCopied(true);
    } catch {
      if (epoch === generation.current) setMessage("复制失败，请手动复制");
    }
  }

  return <div className="modal-backdrop">
    <div className="card modal" role="dialog" aria-modal="true" aria-labelledby="reveal-title" ref={panel}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); close(); }
        if (event.key !== "Tab") return;
        const elements = panel.current?.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), textarea:not(:disabled), [tabindex='0']");
        if (!elements?.length) return;
        const first = elements[0];
        const last = elements[elements.length - 1];
        if (event.shiftKey && (document.activeElement === first || !panel.current?.contains(document.activeElement))) {
          event.preventDefault(); last.focus();
        } else if (!event.shiftKey && (document.activeElement === last || !panel.current?.contains(document.activeElement))) {
          event.preventDefault(); first.focus();
        }
      }}>
      <div className="section-heading"><h2 id="reveal-title">查看当前密钥</h2><button type="button" onClick={close}>关闭</button></div>
      <p className="secondary">输入本地 root 密钥以验证并显示。明文仅在本窗口短时保留，不会保存到浏览器存储。</p>
      {key ? <div className="stack">
        <label htmlFor="revealed-key">当前密钥</label>
        <textarea id="revealed-key" readOnly value={key} rows={3} autoComplete="off" spellCheck={false} />
        <div className="actions"><button type="button" onClick={copy}>{copied ? "已复制" : "复制密钥"}</button>
          <button type="button" onClick={() => { generation.current++; setKey(""); setCopied(false); setMessage("已隐藏密钥"); }}>隐藏 / 清除</button></div>
      </div> : <form onSubmit={reveal} className="stack">
        <label htmlFor="reveal-root">本地 root 密钥</label>
        <input id="reveal-root" type="password" autoComplete="off" value={root} onChange={event => setRoot(event.target.value)} readOnly={busy} required />
        <button className="primary" disabled={busy || !root.trim()}>{busy ? "正在验证…" : "验证并显示"}</button>
      </form>}
      <p className="feedback" role="status">{message}</p>
    </div>
  </div>;
}
