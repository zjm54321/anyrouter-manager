import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import { AlertCircle, Check, Clock, Key, Loader2, Shield, UserPlus, X } from "lucide-react";
import type { CandidateDTO, OperationDTO } from "./api";
import { formatRawQuota } from "./accountsTypes";

export interface AddAccountModalProps {
  isOpen: boolean;
  onClose: () => void;
  onLogin: (credentials: { username: string; password: string }) => Promise<void>;
  candidate: CandidateDTO | null;
  onSaveCandidate: (candidateId: string, keyId: string | null) => Promise<void>;
  operation: OperationDTO | null;
  busy?: boolean;
  candidateExpired?: boolean;
}

export function AddAccountModal({
  isOpen,
  onClose,
  onLogin,
  candidate,
  onSaveCandidate,
  operation,
  busy = false,
  candidateExpired = false,
}: AddAccountModalProps) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [selectedKeyId, setSelectedKeyId] = useState<string>("");
  const [submitting, setSubmitting] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [showLoginForm, setShowLoginForm] = useState(false);

  const generationRef = useRef(0);
  const isAliveRef = useRef(true);
  const pendingSaveRef = useRef<{
    candidateId: string; epoch: number; previousOperationId: string | null;
    operationId: string | null; accepted: boolean;
  } | null>(null);
  const loginAttemptRef = useRef<{ epoch: number; previousOperationId: string | null } | null>(null);
  const [acceptanceVersion, setAcceptanceVersion] = useState(0);

  const dialogRef = useRef<HTMLDivElement>(null);
  const usernameInputRef = useRef<HTMLInputElement>(null);
  const passwordInputRef = useRef<HTMLInputElement>(null);

  const usernameId = useId();
  const passwordId = useId();

  const isLoggingIn =
    submitting ||
    (operation?.kind === "login" && operation?.status === "running") ||
    (busy && !candidate);

  const isSaving =
    submitting ||
    (operation?.kind === "save" && operation?.status === "running");

  // Reset inputs and increment generation on open/close or mount/unmount
  useEffect(() => {
    isAliveRef.current = true;
    let focusTimer: ReturnType<typeof setTimeout> | undefined;
    if (!isOpen) {
      generationRef.current += 1;
      setUsername("");
      setPassword("");
      setLocalError(null);
      setSubmitting(false);
      setSelectedKeyId("");
      setShowLoginForm(false);
      pendingSaveRef.current = null;
      loginAttemptRef.current = null;
    } else {
      generationRef.current += 1;
      setLocalError(null);
      setSubmitting(false);
      if (!candidate || showLoginForm) {
        focusTimer = setTimeout(() => usernameInputRef.current?.focus(), 50);
      }
    }
    return () => {
      isAliveRef.current = false;
      generationRef.current += 1;
      clearTimeout(focusTimer);
      pendingSaveRef.current = null;
      loginAttemptRef.current = null;
    };
  }, [isOpen]);

  // When candidate changes, default to first key or empty
  useEffect(() => {
    if (candidate) {
      setShowLoginForm(false);
      setSelectedKeyId(candidate.keys.length > 0 ? candidate.keys[0].id : "");
    }
  }, [candidate?.id]);

  // Terminal state monitoring for asynchronous save
  useEffect(() => {
    const pending = pendingSaveRef.current;
    if (!isOpen || !isAliveRef.current || !pending || pending.epoch !== generationRef.current) return;
    if (candidate && candidate.id !== pending.candidateId) {
      generationRef.current += 1;
      pendingSaveRef.current = null;
      setSubmitting(false);
      return;
    }
    // useManager exposes only the accepted ID while its save is pending. Bind
    // the NEW operation, including a fast terminal response, never a previous
    // save. Promise resolution is acceptance, not remote completion.
    if (!pending.accepted || operation?.kind !== "save" || operation.id === pending.previousOperationId) return;
    if (!pending.operationId) pending.operationId = operation.id;
    if (operation.id !== pending.operationId) return;
    if (operation.status === "succeeded" && candidate === null) {
      handleClose();
    } else if (operation.status === "failed") {
      pendingSaveRef.current = null;
      setSubmitting(false);
      setLocalError(operation.error?.message || "保存账号失败，请重试");
    }
  }, [candidate, operation, isOpen, acceptanceVersion]);

  // Scoped error display: local errors, or failed operations for login/save
  const displayedError =
    localError ||
    (loginAttemptRef.current?.epoch === generationRef.current &&
      operation?.id !== loginAttemptRef.current?.previousOperationId &&
      operation?.status === "failed" && operation.kind === "login"
      ? operation.error?.message || "操作失败，请重试"
      : null);

  function handleClose() {
    generationRef.current += 1;
    pendingSaveRef.current = null;
    loginAttemptRef.current = null;
    setUsername("");
    setPassword("");
    setLocalError(null);
    setSubmitting(false);
    setSelectedKeyId("");
    setShowLoginForm(false);
    onClose();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      handleClose();
      return;
    }

    if (event.key !== "Tab") return;
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href], [tabindex="0"]'
    );
    if (!focusable?.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function handleLoginSubmit(e: FormEvent) {
    e.preventDefault();
    const cleanUser = username.trim();
    if (!cleanUser || !password || isLoggingIn) return;

    const creds = { username: cleanUser, password };
    // Immediately clear password from memory
    setPassword("");
    const epoch = ++generationRef.current;
    loginAttemptRef.current = { epoch, previousOperationId: operation?.id ?? null };
    setSubmitting(true);
    setLocalError(null);

    try {
      await onLogin(creds);
      if (!isAliveRef.current || epoch !== generationRef.current) return;
    } catch (err: unknown) {
      if (!isAliveRef.current || epoch !== generationRef.current) return;
      setLocalError(err instanceof Error ? err.message : "登录请求失败，请检查网络或重试");
    } finally {
      if (isAliveRef.current && epoch === generationRef.current) {
        setSubmitting(false);
      }
    }
  }

  async function handleSaveSubmit(e: FormEvent) {
    e.preventDefault();
    if (!candidate || isSaving) return;

    const epoch = ++generationRef.current;
    setSubmitting(true);
    setLocalError(null);
    loginAttemptRef.current = null;
    const pending = { candidateId: candidate.id, epoch, previousOperationId: operation?.id ?? null,
      operationId: null, accepted: false };
    pendingSaveRef.current = pending;

    try {
      await onSaveCandidate(candidate.id, selectedKeyId ? selectedKeyId : null);
      if (!isAliveRef.current || epoch !== generationRef.current || pendingSaveRef.current !== pending) return;
      pending.accepted = true;
      setAcceptanceVersion(value => value + 1);
    } catch (err: unknown) {
      if (!isAliveRef.current || epoch !== generationRef.current || pendingSaveRef.current !== pending) return;
      pendingSaveRef.current = null;
      setLocalError(err instanceof Error ? err.message : "保存账号失败，请重试");
      setSubmitting(false);
    }
  }

  if (!isOpen) return null;

  return (
    <div className="modal-backdrop" onClick={e => e.target === e.currentTarget && handleClose()}>
      <div
        className="card modal-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-account-title"
        ref={dialogRef}
        onKeyDown={handleKeyDown}
      >
        <div className="modal-header">
          <div className="modal-title-group">
            <div className="modal-icon-badge">
              <UserPlus size={18} strokeWidth={2} />
            </div>
            <div>
              <h2 id="modal-account-title" className="modal-title">
                {candidate && !showLoginForm ? "保存候选账号" : "添加账号"}
              </h2>
            </div>
          </div>
          <button
            type="button"
            className="btn-icon"
            onClick={handleClose}
            aria-label="关闭对话框"
          >
            <X size={18} />
          </button>
        </div>

        <div className="modal-body">
          {displayedError && (
            <div className="alert alert-error" role="alert">
              <AlertCircle size={16} className="alert-icon" />
              <div className="alert-content">{displayedError}</div>
            </div>
          )}

          {candidate && !candidateExpired && !showLoginForm ? (
            <form className="stack" onSubmit={handleSaveSubmit}>
              {candidateExpired && (
                <div className="alert alert-warn" role="alert">
                  <Clock size={16} className="alert-icon" />
                  <div className="alert-content">
                    候选账号已过期，请重新登录。
                  </div>
                </div>
              )}

              {candidate.keys.length === 0 && (
                <div className="alert alert-warn" role="alert">
                  <AlertCircle size={16} className="alert-icon" />
                  <div className="alert-content">
                    未选择密钥，仅管理账号。
                  </div>
                </div>
              )}

              <div className="candidate-info-card">
                <div className="candidate-prop-row">
                  <span className="prop-label">账号名称</span>
                  <span className="prop-value font-medium">{candidate.username}</span>
                </div>
                <div className="candidate-prop-row">
                  <span className="prop-label">可用额度</span>
                  <span className="prop-value font-semibold text-teal tabular-nums">
                    {formatRawQuota(candidate.balance.quota_raw)}
                  </span>
                </div>
                <div className="candidate-prop-row">
                  <span className="prop-label">已用额度</span>
                  <span className="prop-value text-secondary tabular-nums">
                    {formatRawQuota(candidate.balance.used_quota_raw)}
                  </span>
                </div>
              </div>

              <div className="field">
                <label className="field-label">关联网关密钥（可选）</label>
                <div className="candidate-key-options" role="radiogroup" aria-label="选择关联密钥">
                  {candidate.keys.map(k => (
                    <label
                      key={k.id}
                      className={`key-option ${selectedKeyId === k.id ? "selected" : ""}`}
                    >
                      <input
                        type="radio"
                        name="candidate-key"
                        value={k.id}
                        checked={selectedKeyId === k.id}
                        onChange={() => setSelectedKeyId(k.id)}
                        disabled={isSaving || candidateExpired}
                      />
                      <div className="key-option-info">
                        <span className="key-option-name font-medium">{k.name}</span>
                        <code className="masked-key text-xs font-mono">{k.masked}</code>
                        {!k.enabled && <span className="badge badge-warn text-xs">已停用</span>}
                      </div>
                    </label>
                  ))}
                  <label className={`key-option ${selectedKeyId === "" ? "selected" : ""}`}>
                    <input
                      type="radio"
                      name="candidate-key"
                      value=""
                      checked={selectedKeyId === ""}
                      onChange={() => setSelectedKeyId("")}
                      disabled={isSaving || candidateExpired}
                    />
                    <div className="key-option-info">
                      <span className="key-option-name">不选择密钥</span>
                      <span className="text-secondary text-xs">未选择密钥，仅管理账号</span>
                    </div>
                  </label>
                </div>
              </div>

              <div className="modal-footer">
                <button
                  type="button"
                  className="btn btn-ghost"
                  onClick={() => setShowLoginForm(true)}
                  disabled={isSaving}
                >
                  重新登录
                </button>
                <div className="button-group-right">
                  <button
                    type="button"
                    className="btn btn-secondary"
                    onClick={handleClose}
                    disabled={isSaving}
                  >
                    取消
                  </button>
                  <button
                    type="submit"
                    className="btn btn-primary"
                    disabled={isSaving || candidateExpired}
                  >
                    {isSaving ? (
                      <>
                        <Loader2 size={16} className="spin" />
                        <span>正在保存…</span>
                      </>
                    ) : (
                      <>
                        <Check size={16} />
                        <span>保存账号</span>
                      </>
                    )}
                  </button>
                </div>
              </div>
            </form>
          ) : (
            <form className="stack" onSubmit={handleLoginSubmit}>
              {candidateExpired && (
                <div className="alert alert-warn" role="alert">
                  <AlertCircle size={16} className="alert-icon" />
                  <div className="alert-content">候选账号已过期，请重新登录</div>
                </div>
              )}

              <div className="field">
                <label htmlFor={usernameId} className="field-label">
                  用户名
                </label>
                <input
                  id={usernameId}
                  ref={usernameInputRef}
                  type="text"
                  className="text-input"
                  required
                  autoComplete="username"
                  placeholder="用户名 / 邮箱"
                  value={username}
                  onChange={e => setUsername(e.target.value)}
                  disabled={isLoggingIn}
                />
              </div>

              <div className="field">
                <label htmlFor={passwordId} className="field-label">
                  密码
                </label>
                <input
                  id={passwordId}
                  ref={passwordInputRef}
                  type="password"
                  className="text-input"
                  required
                  autoComplete="current-password"
                  placeholder="••••••••••••"
                  value={password}
                  onChange={e => setPassword(e.target.value)}
                  disabled={isLoggingIn}
                />
              </div>

              {isLoggingIn && (
                <div className="login-progress-banner">
                  <Loader2 size={16} className="spin text-teal" />
                  <span className="text-sm">
                    {operation?.phase || "正在登录…"}
                  </span>
                </div>
              )}

              <div className="modal-footer">
                {candidate && (
                  <button
                    type="button"
                    className="btn btn-ghost"
                    onClick={() => setShowLoginForm(false)}
                    disabled={isLoggingIn}
                  >
                    返回候选账号
                  </button>
                )}
                <div className="button-group-right">
                  <button
                    type="button"
                    className="btn btn-secondary"
                    onClick={handleClose}
                    disabled={isLoggingIn}
                  >
                    取消
                  </button>
                  <button
                    type="submit"
                    className="btn btn-primary"
                    disabled={isLoggingIn || !username.trim() || !password}
                  >
                    {isLoggingIn ? (
                      <>
                        <Loader2 size={16} className="spin" />
                        <span>正在登录…</span>
                      </>
                    ) : (
                      <>
                        <Shield size={16} />
                        <span>登录</span>
                      </>
                    )}
                  </button>
                </div>
              </div>
            </form>
          )}
        </div>
      </div>
    </div>
  );
}
