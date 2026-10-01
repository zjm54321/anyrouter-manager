import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, failureText, request } from "./api";
import type { AccountState } from "./api";

const empty: AccountState = { active: null, candidate: null, operation: null };

export function useManager() {
  const [session, setSession] = useState<"loading" | "locked" | "open">("loading");
  const [state, setState] = useState<AccountState>(empty);
  const [notice, setNotice] = useState("");
  const [sending, setSending] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [candidateExpired, setCandidateExpired] = useState(false);
  const generation = useRef(0);
  const loadSequence = useRef(0);
  const controllers = useRef(new Set<AbortController>());

  const abortAll = useCallback(() => {
    generation.current++;
    controllers.current.forEach(controller => controller.abort());
    controllers.current.clear();
  }, []);

  const controller = useCallback(() => {
    const next = new AbortController();
    controllers.current.add(next);
    return next;
  }, []);

  const lock = useCallback(() => {
    abortAll();
    setSession("locked");
    setState(empty);
    setSending(false);
    setLoadError(false);
    setCandidateExpired(false);
  }, [abortAll]);

  const load = useCallback(async () => {
    const epoch = generation.current;
    const sequence = ++loadSequence.current;
    const task = controller();
    try {
      const data = await request<AccountState>("/api/account", { signal: task.signal });
      if (epoch !== generation.current || sequence !== loadSequence.current) return;
      setState(data);
      setSession("open");
      setLoadError(false);
      setCandidateExpired(false);
      setNotice("");
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted || sequence !== loadSequence.current) return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
        setNotice("请输入本地 root 密钥以建立管理会话");
      } else {
        setLoadError(true);
        setNotice(failureText(error));
      }
    } finally {
      controllers.current.delete(task);
    }
  }, [controller, lock]);

  useEffect(() => {
    void load();
    return abortAll;
  }, [abortAll, load]);

  useEffect(() => {
    if (session !== "open" || state.operation?.status !== "running" || loadError) return;
    const timer = window.setTimeout(() => void load(), 1000);
    return () => window.clearTimeout(timer);
  }, [session, state, loadError, load]);

  async function unlock(root: string) {
    const epoch = generation.current;
    const task = controller();
    setSending(true);
    setNotice("");
    try {
      const pending = request<void>("/api/admin/session", {
        method: "POST", headers: { Authorization: `Bearer ${root}` }, signal: task.signal,
      });
      root = "";
      await pending;
      if (epoch !== generation.current) return;
      await load();
    } catch (error) {
      if (epoch === generation.current && !task.signal.aborted) {
        setNotice(error instanceof ApiError && error.status === 401 ? "本地 root 密钥不正确，请重试" : failureText(error));
      }
    } finally {
      root = "";
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }

  async function perform(kind: "login" | "activate" | "refresh", body?: object) {
    const epoch = generation.current;
    const task = controller();
    setSending(true);
    setNotice("");
    try {
      const pending = request<{ operation_id: string }>(`/api/account/${kind}`, {
        method: "POST", signal: task.signal,
        ...(body ? { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) } : {}),
      });
      body = undefined;
      await pending;
      if (epoch !== generation.current) return;
      // Accepted jobs need an immediate snapshot even if the previous operation was null.
      await load();
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.detail.code === "candidate_expired") setCandidateExpired(true);
      // Only GET /account 401 locks the UI. POST auth failures are inline and retryable.
      setNotice(error instanceof ApiError && error.status === 401
        ? "管理请求未通过鉴权，请重试或退出后重新建立管理会话"
        : failureText(error));
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }

  async function logout() {
    lock();
    setNotice("已清除本页账号信息，正在注销管理会话");
    const epoch = generation.current;
    const task = controller();
    setSending(true);
    try {
      await request<void>("/api/admin/session", { method: "DELETE", signal: task.signal });
      if (epoch === generation.current) setNotice("已退出管理会话");
    } catch {
      if (epoch === generation.current && !task.signal.aborted) {
        setNotice("已清除本页账号信息，但服务端会话注销失败；请重试注销，关闭页面不会撤销服务端会话");
      }
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }

  return {
    session, state, notice, sending, loadError, candidateExpired, load, unlock, perform, logout,
    busy: sending || state.operation?.status === "running",
  };
}
