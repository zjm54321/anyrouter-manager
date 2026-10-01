import { useCallback, useEffect, useRef, useState } from "react";
import {
  ApiError,
  failureText,
  fetchCheckin,
  request,
  runCheckin,
  updateCheckinSettings,
} from "./api";
import type { AccountState, CheckinFullStatus } from "./api";

const empty: AccountState = { active: null, candidate: null, operation: null };

export function useManager() {
  const [session, setSession] = useState<"loading" | "locked" | "open">("loading");
  const [state, setState] = useState<AccountState>(empty);
  const [notice, setNotice] = useState("");
  const [sending, setSending] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [candidateExpired, setCandidateExpired] = useState(false);

  // Checkin state
  const [checkin, setCheckin] = useState<CheckinFullStatus | null>(null);
  const [checkinError, setCheckinError] = useState("");
  const [checkinRunError, setCheckinRunError] = useState("");
  const [checkinSaving, setCheckinSaving] = useState(false);

  const generation = useRef(0);
  const loadSequence = useRef(0);
  const checkinSequence = useRef(0);
  const controllers = useRef(new Set<AbortController>());
  const sessionRef = useRef<"loading" | "locked" | "open">("loading");
  sessionRef.current = session;

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
    sessionRef.current = "locked";
    setState(empty);
    setCheckin(null);
    setCheckinError("");
    setCheckinRunError("");
    setCheckinSaving(false);
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
      sessionRef.current = "open";
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

  const loadCheckin = useCallback(async () => {
    if (sessionRef.current !== "open") return;
    const epoch = generation.current;
    const sequence = ++checkinSequence.current;
    const task = controller();
    try {
      const data = await fetchCheckin(task.signal);
      if (epoch !== generation.current || sequence !== checkinSequence.current) return;
      setCheckin(data);
      setCheckinError("");
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted || sequence !== checkinSequence.current) return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
        setNotice("请输入本地 root 密钥以建立管理会话");
      } else {
        setCheckinError(failureText(error));
      }
    } finally {
      controllers.current.delete(task);
    }
  }, [controller, lock]);

  // Initial account load
  useEffect(() => {
    void load();
    return abortAll;
  }, [abortAll, load]);

  // Poll account while any operation is running
  useEffect(() => {
    if (session !== "open" || state.operation?.status !== "running" || loadError) return;
    const timer = window.setTimeout(() => void load(), 1000);
    return () => window.clearTimeout(timer);
  }, [session, state, loadError, load]);

  // Trigger checkin fetch on session open or active account change
  useEffect(() => {
    if (session !== "open") return;
    void loadCheckin();
  }, [session, state.active?.revision, loadCheckin]);

  // Refresh checkin when an operation finishes running
  const prevOpRunning = useRef(false);
  useEffect(() => {
    const isRunning = state.operation?.status === "running";
    if (prevOpRunning.current && !isRunning && session === "open") {
      void loadCheckin();
    }
    prevOpRunning.current = isRunning;
  }, [state.operation?.status, session, loadCheckin]);

  // Refresh checkin when tab becomes visible
  useEffect(() => {
    if (session !== "open") return;
    const onVisibility = () => {
      if (document.visibilityState === "visible") {
        void loadCheckin();
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [session, loadCheckin]);

  // Bounded timer for next scheduled run or 08:00 CST boundary
  useEffect(() => {
    if (session !== "open") return;
    const now = Date.now();
    let delay: number | null = null;

    if (checkin?.next_run_at) {
      const nextRunTime = new Date(checkin.next_run_at).getTime();
      const diff = nextRunTime - now;
      if (diff > 0 && diff <= 86400000) {
        delay = diff + 1000;
      }
    }

    const nowDate = new Date(now);
    const nextUtcMidnight = new Date(Date.UTC(nowDate.getUTCFullYear(), nowDate.getUTCMonth(), nowDate.getUTCDate() + 1, 0, 0, 1));
    const diffTo08 = nextUtcMidnight.getTime() - now;
    if (diffTo08 > 0 && diffTo08 <= 86400000) {
      delay = delay === null ? diffTo08 : Math.min(delay, diffTo08);
    }

    if (delay === null) return;
    const timer = window.setTimeout(() => {
      void loadCheckin();
    }, delay);
    return () => window.clearTimeout(timer);
  }, [session, checkin?.next_run_at, loadCheckin]);

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
      await load();
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.detail.code === "candidate_expired") setCandidateExpired(true);
      setNotice(error instanceof ApiError && error.status === 401
        ? "管理请求未通过鉴权，请重试或退出后重新建立管理会话"
        : failureText(error));
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }

  async function saveCheckinSettings(settings: { enabled: boolean; time: string }) {
    const epoch = generation.current;
    const task = controller();
    setCheckinSaving(true);
    setCheckinRunError("");
    try {
      const data = await updateCheckinSettings(settings, task.signal);
      if (epoch !== generation.current) return;
      setCheckin(data);
      setCheckinError("");
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
        setNotice("请输入本地 root 密钥以建立管理会话");
      } else {
        setCheckinRunError(failureText(error));
        throw error;
      }
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setCheckinSaving(false);
    }
  }

  async function runCheckinAction(options: { confirmRetry: boolean }) {
    const epoch = generation.current;
    const task = controller();
    setSending(true);
    setCheckinRunError("");
    try {
      const res = await runCheckin(options.confirmRetry, task.signal);
      if (epoch !== generation.current) return;
      if (res.already_recorded) {
        await loadCheckin();
      } else {
        await load();
        await loadCheckin();
      }
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError) {
        if (error.status === 401) {
          lock();
          setNotice("请输入本地 root 密钥以建立管理会话");
          return;
        }
        if (error.detail.code === "operation_in_progress") {
          await load();
          return;
        }
        if (error.detail.code === "checkin_retry_confirmation_required") {
          setCheckinRunError("当前周期签到结果处于未确认状态，重试可能导致重复提交，请确认后重试");
          return;
        }
      }
      setCheckinRunError(failureText(error));
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
    checkin, checkinError, checkinRunError, checkinSaving,
    loadCheckin, saveCheckinSettings, runCheckinAction,
  };
}
