import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { errorText, phases } from "./api";
import type { AccountBalance, CandidateAccount } from "./api";
import { RevealDialog } from "./RevealDialog";
import { useManager } from "./useManager";

function date(value: string) {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString("zh-CN", { hour12: false });
}

function Balance({ balance }: { balance: AccountBalance }) {
  return <div>
    <dl className="balance-grid">
      <div className="subcard"><dt>剩余额度（原始单位）</dt><dd className="amount">{balance.quota_raw}</dd></div>
      <div className="subcard"><dt>已用额度</dt><dd className="amount">{balance.used_quota_raw}</dd></div>
    </dl>
    <p className="secondary small numeric">余额读取时间：{date(balance.fetched_at)}</p>
  </div>;
}

function GatewayBanner() {
  const url = import.meta.env.DEV ? "http://127.0.0.1:8080/v1" : `${window.location.origin}/v1`;
  const [feedback, setFeedback] = useState("");
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    if (!feedback) return;
    const timer = window.setTimeout(() => setFeedback(""), 2000);
    return () => window.clearTimeout(timer);
  }, [feedback]);
  return <section className="card gateway" aria-labelledby="gateway-title">
    <div className="section-heading"><h2 id="gateway-title">本地网关</h2><button type="button" onClick={async () => {
      try { await navigator.clipboard.writeText(url); if (mounted.current) setFeedback("已复制网关地址"); }
      catch { if (mounted.current) setFeedback("复制失败，请手动复制"); }
    }}>复制地址</button></div>
    <code className="gateway-url">{url}</code>
    <p className="secondary">客户端使用本地 root 密钥；后端替换为当前账号的上游密钥</p>
    {import.meta.env.DEV && <p className="secondary small">开发环境网关位于后端 8080 端口，不是当前 Vite 页面端口。</p>}
    <span className="feedback" role="status">{feedback}</span>
  </section>;
}

function Candidate({ candidate, busy, replacing, onActivate }: {
  candidate: CandidateAccount; busy: boolean; replacing: boolean;
  onActivate: (candidateId: string, keyId: string) => void;
}) {
  const [keyId, setKeyId] = useState("");
  return <div className="stack">
    <div><h3>{candidate.username}</h3><p className="secondary small">上游用户 ID：{candidate.upstream_user_id}</p></div>
    <Balance balance={candidate.balance} />
    <fieldset disabled={busy} className="key-options">
      <legend>选择已有密钥</legend>
      {candidate.keys.map(key => <label className={`key-option ${keyId === key.id ? "selected" : ""}`} key={key.id}>
        <input type="radio" name="candidate-key" value={key.id} checked={keyId === key.id} onChange={() => setKeyId(key.id)} />
        <span><span className="key-name">{key.name || "未命名密钥"}</span><code>{key.masked}</code><span className="secondary small">ID：{key.id}</span></span>
      </label>)}
    </fieldset>
    {!candidate.keys.length && <p className="inline-warning">未找到已有密钥，请先在 AnyRouter 创建后重试</p>}
    <p className="secondary small numeric">候选账号暂存 15 分钟，到期时间：{date(candidate.expires_at)}</p>
    {replacing && <p className="inline-warning">启用后将替换当前账号</p>}
    <button className="primary" type="button" disabled={busy || !keyId || !candidate.keys.some(key => key.id === keyId)}
      onClick={() => onActivate(candidate.id, keyId)}>确认启用所选密钥</button>
  </div>;
}

export function App() {
  const manager = useManager();
  const { state, session, busy } = manager;
  const [root, setRoot] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [revealRevision, setRevealRevision] = useState<string | null>(null);
  const [reloginCandidate, setReloginCandidate] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (session !== "open") { setUsername(""); setPassword(""); setRoot(""); setRevealRevision(null); setReloginCandidate(null); }
  }, [session]);
  useEffect(() => {
    setRevealRevision(null);
  }, [state.active?.revision]);
  useEffect(() => {
    if (!state.candidate) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [state.candidate]);

  const expired = manager.candidateExpired || !!(state.candidate && new Date(state.candidate.expires_at).getTime() <= now);
  function unlock(event: FormEvent) {
    event.preventDefault();
    const pending = manager.unlock(root);
    setRoot("");
    void pending;
  }
  function login(event: FormEvent) {
    event.preventDefault();
    const pending = manager.perform("login", { username, password });
    setPassword("");
    void pending;
  }

  if (session !== "open") return <main className="locked-layout">
    <section className="card locked-card" aria-labelledby="app-title">
      <h1 id="app-title">AnyRouter Manager</h1>
      <p className="secondary">使用本地 root 密钥建立管理会话</p>
      <p className="notice" role="status" aria-live="polite">{manager.notice || (session === "loading" ? "正在读取管理会话…" : "管理页面已锁定")}</p>
      {session === "locked" && <form className="stack" onSubmit={unlock}>
        <label htmlFor="admin-root">本地 root 密钥</label>
        <input id="admin-root" type="password" autoComplete="off" required value={root} disabled={manager.sending} onChange={event => setRoot(event.target.value)} />
        <button className="primary" disabled={manager.sending || !root.trim()}>{manager.sending ? "正在建立会话…" : "进入管理页面"}</button>
      </form>}
      {manager.loadError && <button type="button" onClick={() => void manager.load()}>重试读取状态</button>}
      {session === "locked" && manager.notice.includes("注销失败") && <button type="button" disabled={manager.sending} onClick={() => void manager.logout()}>重试注销</button>}
      <p className="secondary small">密钥仅用于本次验证，不保存在浏览器存储；管理会话由后端维护。</p>
      <a href="https://anyrouter.top" target="_blank" rel="noreferrer">AnyRouter 上游：anyrouter.top</a>
    </section>
  </main>;

  const operation = state.operation;
  const operationNotice = operation?.status === "running" ? phases[operation.phase]
    : operation?.status === "failed" ? (operation.error ? errorText(operation.error) : "操作未完成，请重试")
    : operation?.status === "succeeded" ? "操作已完成" : "";
  const revealOpen = !!state.active && revealRevision === state.active.revision;
  return <><main className="dashboard" inert={revealOpen} aria-hidden={revealOpen || undefined}>
    <header className="page-header">
      <div><h1>AnyRouter Manager</h1><a href="https://anyrouter.top" target="_blank" rel="noreferrer">固定上游：https://anyrouter.top</a></div>
      <button type="button" onClick={() => { setRevealRevision(null); setPassword(""); setUsername(""); void manager.logout(); }}>退出管理</button>
    </header>
    <div className={`operation-notice ${manager.loadError || operation?.status === "failed" ? "error-notice" : ""}`} role="status" aria-live="polite" aria-atomic="true">
      {manager.notice || operationNotice || "管理会话已建立"}
      {manager.loadError && <button type="button" onClick={() => void manager.load()}>重试读取状态</button>}
    </div>
    <GatewayBanner />
    <section className="card" aria-labelledby="active-title">
      <div className="section-heading"><h2 id="active-title">当前账号</h2>{state.active && <span className="badge success">已配置</span>}</div>
      {state.active ? <div className="stack">
        <div><h3>{state.active.username}</h3><p className="secondary small">上游用户 ID：{state.active.upstream_user_id}</p></div>
        <Balance balance={state.active.balance} />
        <div className="subcard stack compact">
          <span className="secondary small">当前密钥 · {state.active.selected_key.name || "未命名密钥"}</span>
          <code className="masked-key">{state.active.selected_key.masked}</code>
          <span className="secondary small numeric">版本：{state.active.revision} · 启用时间：{date(state.active.activated_at)}</span>
        </div>
        <div className="actions"><button type="button" disabled={busy} onClick={() => void manager.perform("refresh")}>刷新余额</button>
          <button type="button" disabled={busy} onClick={() => setRevealRevision(state.active!.revision)}>查看密钥</button></div>
      </div> : <div className="empty-state"><h3>尚未配置账号</h3><p className="secondary">登录 AnyRouter 并选择一个已有密钥后，才能配置本地网关。</p></div>}
    </section>
    <section className="card" aria-labelledby="candidate-title">
      <div className="section-heading"><h2 id="candidate-title">{state.active ? "更换账号" : "添加账号"}</h2>{state.candidate && !expired && <span className="badge">待确认</span>}</div>
      <p className="secondary">登录后读取余额与已有密钥；确认启用前不会替换当前账号。系统不会创建新密钥。</p>
      {state.candidate && !expired && reloginCandidate !== state.candidate.id ? <>
        <Candidate key={state.candidate.id} candidate={state.candidate} busy={busy} replacing={!!state.active}
          onActivate={(candidate_id, key_id) => void manager.perform("activate", { candidate_id, key_id })} />
        <button type="button" disabled={busy} onClick={() => setReloginCandidate(state.candidate!.id)}>重新登录并读取</button>
      </>
        : <>
          {expired && <p className="inline-warning">候选账号已过期，请重新登录</p>}
          <form className="stack" onSubmit={login}>
            <div className="field"><label htmlFor="username">AnyRouter 用户名</label><input id="username" autoComplete="username" value={username} onChange={event => setUsername(event.target.value)} disabled={busy} required /></div>
            <div className="field"><label htmlFor="password">AnyRouter 密码</label><input id="password" type="password" autoComplete="off" value={password} onChange={event => setPassword(event.target.value)} disabled={busy} required /></div>
            <p className="secondary small">密码仅本次使用，不会保存；上游会话仅由后端维护。</p>
            <button className="primary" disabled={busy || !username.trim() || !password}>{busy ? "正在处理…" : "登录并读取已有密钥"}</button>
          </form>
        </>}
    </section>
  </main>
    {state.active && revealOpen && <RevealDialog key={state.active.revision} active={state.active} onClose={() => setRevealRevision(null)} />}
  </>;
}
