import { useCallback, useEffect, useRef, useState } from "react";
import {
  adminLogin,
  adminLogout,
  ApiError,
  clearRequestLogs as apiClearRequestLogs,
  clearSystemLogs as apiClearSystemLogs,
  errorText,
  failureText,
  fetchAccountCheckin,
  fetchAccounts,
  fetchGlobalCheckin,
  fetchLogSettings,
  fetchRequestLogs,
  fetchSystemLogs as apiFetchSystemLogs,
  loginAccount,
  refreshAccount as apiRefreshAccount,
  revealAccountKey,
  routeAccount as apiRouteAccount,
  runAccountCheckin as apiRunAccountCheckin,
  saveAccount,
  selectAccountKey as apiSelectAccountKey,
  updateGlobalCheckinSettings,
  updateLogSettings,
} from "./api";
import type {
  AccountCheckinResponse,
  AccountsResponse,
  GlobalCheckinResponse,
  LogSettings,
  OperationDTO,
  RequestLogItem,
  SystemLogsResponse,
} from "./api";

const emptyState: AccountsResponse = {
  accounts: [],
  route_account_id: null,
  candidate: null,
  operation: null,
};

const statusUnavailable = "暂时无法确认任务状态，请重试读取状态；不要重复提交操作";
const monitorWindowMs = 180_000;
const quotaPreferenceKey = "anyrouter-manager.quota-auto-refresh.v1";
function quotaPreferences(): { enabled: boolean; minutes: 5 | 10 } {
  try {
    const saved = JSON.parse(localStorage.getItem(quotaPreferenceKey) || "null");
    return { enabled: typeof saved?.enabled === "boolean" ? saved.enabled : true,
      minutes: saved?.minutes === 5 ? 5 : 10 };
  } catch { return { enabled: true, minutes: 10 }; }
}
type PendingOperation = {
  id: string;
  kind: OperationDTO["kind"];
  accountId: string | null;
  startedAt: number;
  missingReads: number;
};
type OperationAdmission = { pending: PendingOperation | null };

export function useManager() {
  const [session, setSession] = useState<"loading" | "locked" | "open">("loading");
  const [state, setState] = useState<AccountsResponse>(emptyState);
  const [pendingRouteAccountId, setPendingRouteAccountId] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const [sending, setSendingState] = useState(false);
  const sendingRef = useRef(false);
  const setSending = useCallback((value: boolean) => {
    sendingRef.current = value;
    setSendingState(value);
  }, []);
  const [loadError, setLoadError] = useState(false);
  const [candidateExpired, setCandidateExpired] = useState(false);

  // Global and per-account checkin state
  const [globalCheckin, setGlobalCheckin] = useState<GlobalCheckinResponse | null>(null);
  const [accountCheckin, setAccountCheckin] = useState<AccountCheckinResponse | null>(null);
  const [selectedAccountId, setSelectedAccountId] = useState<string | null>(null);
  const [checkinError, setCheckinError] = useState("");
  const [checkinRunError, setCheckinRunError] = useState("");
  const [checkinSaving, setCheckinSaving] = useState(false);

  // Request logs state
  const [logs, setLogs] = useState<RequestLogItem[]>([]);
  const [logsLoading, setLogsLoading] = useState(false);
  const [logsError, setLogsError] = useState<string | null>(null);

  // Per-account refresh tracking
  const [refreshingAccountId, setRefreshingAccountId] = useState<string | null>(null);
  const [accountRefreshError, setAccountRefreshError] = useState<Record<string, string>>({});
  const [quotaPreference, setQuotaPreference] = useState(quotaPreferences);
  const quotaPreferenceChanged = useRef(false);
  const quotaAutoRefreshEnabled = quotaPreference.enabled;
  const quotaRefreshIntervalMinutes = quotaPreference.minutes;
  const operationAdmission = useRef<OperationAdmission | null>(null);
  const quotaOpenedAt = useRef<number | null>(null);
  const quotaBackoff = useRef(new Map<string, number>());
  const quotaPaused = useRef(new Map<string, string>());
  const quotaRound = useRef({ startedAt: 0, seen: new Set<string>() });
  const quotaReconciling = useRef(false);
  const [quotaVisible, setQuotaVisible] = useState(document.visibilityState === "visible");
  const [quotaWake, setQuotaWake] = useState(0);
  const latestState = useRef(state);
  latestState.current = state;
  const setQuotaAutoRefreshEnabled = useCallback((enabled: boolean) => {
    quotaPreferenceChanged.current = true;
    setQuotaPreference(previous => ({ ...previous, enabled }));
  }, []);
  const setQuotaRefreshIntervalMinutes = useCallback((minutes: 5 | 10) => {
    if (minutes === 5 || minutes === 10) {
      quotaPreferenceChanged.current = true;
      setQuotaPreference(previous => ({ ...previous, minutes }));
    }
  }, []);
  useEffect(() => {
    if (!quotaPreferenceChanged.current) return;
    try { localStorage.setItem(quotaPreferenceKey, JSON.stringify(quotaPreference)); } catch { /* optional preference */ }
  }, [quotaPreference]);

  // System logs & settings state
  const [logSettings, setLogSettings] = useState<LogSettings | null>(null);
  const [systemLogs, setSystemLogs] = useState<SystemLogsResponse | null>(null);
  const [systemLogsLoading, setSystemLogsLoading] = useState(false);
  const [systemLogsError, setSystemLogsError] = useState<string | null>(null);

  const generation = useRef(0);
  const accountsVersion = useRef(0);
  const accountsFlight = useRef<{ epoch: number; promise: Promise<void>; again: boolean } | null>(null);
  const pendingRef = useRef<PendingOperation | null>(null);
  const [pendingOperation, setPendingOperation] = useState<PendingOperation | null>(null);
  const monitorPaused = useRef(false);
  const [completedOperation, setCompletedOperation] = useState<OperationDTO | null>(null);
  const observedRunning = useRef<string | null>(null);
  const selectedAccountRef = useRef<string | null>(null);
  const checkinSequence = useRef(0);
  const accountCheckinSequence = useRef(0);
  const logsSequence = useRef(0);
  const systemLogsSequence = useRef(0);
  const logSettingsSequence = useRef(0);
  const controllers = useRef(new Set<AbortController>());
  const sessionRef = useRef<"loading" | "locked" | "open">("loading");
  sessionRef.current = session;

  const abortAll = useCallback(() => {
    generation.current++;
    accountsFlight.current = null;
    pendingRef.current = null;
    observedRunning.current = null;
    monitorPaused.current = false;
    operationAdmission.current = null;
    quotaOpenedAt.current = null;
    quotaBackoff.current.clear();
    quotaPaused.current.clear();
    quotaRound.current = { startedAt: 0, seen: new Set() };
    quotaReconciling.current = false;
    controllers.current.forEach(controller => controller.abort());
    controllers.current.clear();
  }, []);

  const createController = useCallback(() => {
    const next = new AbortController();
    controllers.current.add(next);
    return next;
  }, []);

  const lock = useCallback(() => {
    abortAll();
    setSession("locked");
    sessionRef.current = "locked";
    setState(emptyState);
    setPendingRouteAccountId(null);
    setGlobalCheckin(null);
    setAccountCheckin(null);
    setSelectedAccountId(null);
    selectedAccountRef.current = null;
    setPendingOperation(null);
    setCompletedOperation(null);
    setLogs([]);
    setLogsError(null);
    setLogsLoading(false);
    setRefreshingAccountId(null);
    setAccountRefreshError({});
    setLogSettings(null);
    setSystemLogs(null);
    setSystemLogsError(null);
    setSystemLogsLoading(false);
    setCheckinError("");
    setCheckinRunError("");
    setCheckinSaving(false);
    setSending(false);
    setLoadError(false);
    setCandidateExpired(false);
  }, [abortAll]);

  // Reserve synchronously before allocating a request: React's busy state may
  // not have rendered yet when another manual action or timer fires.
  function reserveOperation(): OperationAdmission | null {
    if (sessionRef.current !== "open" || sendingRef.current || operationAdmission.current
        || pendingRef.current || latestState.current.operation?.status === "running" || monitorPaused.current) return null;
    const owner = { pending: null };
    operationAdmission.current = owner;
    setSending(true);
    return owner;
  }

  function finishSubmission(owner: OperationAdmission) {
    // A matched terminal read can admit a new operation before this finally.
    // Likewise logout/reopening invalidates the old owner, not just its request.
    if (operationAdmission.current !== owner) return;
    if (!owner.pending) {
      operationAdmission.current = null;
      setQuotaWake(value => value + 1);
    }
    setSending(false);
  }

  const trackOperation = useCallback((owner: OperationAdmission, id: string, kind: OperationDTO["kind"], accountId: string | null = null) => {
    // Acceptance is not completion. Invalidate only snapshots started BEFORE
    // this POST, never an in-flight read merely because another poll wants it.
    accountsVersion.current++;
    if (!id) throw new Error(statusUnavailable);
    const pending = { id, kind, accountId, startedAt: Date.now(), missingReads: 0 };
    owner.pending = pending;
    pendingRef.current = pending;
    setPendingOperation(pending);
    monitorPaused.current = false;
    setLoadError(false);
  }, []);

  // One flight, with at most one queued fresh read. Action-triggered reloads
  // join this flight rather than starving responses with newer sequence IDs.
  const load = useCallback((): Promise<void> => {
    const epoch = generation.current;
    const current = accountsFlight.current;
    if (current?.epoch === epoch) {
      current.again = true;
      return current.promise;
    }
    if (monitorPaused.current) {
      monitorPaused.current = false;
      if (pendingRef.current) {
        pendingRef.current.startedAt = Date.now();
        pendingRef.current.missingReads = 0;
      }
    }
    const flight = { epoch, again: false, promise: Promise.resolve() };
    accountsFlight.current = flight;
    const read = async () => {
    const version = accountsVersion.current;
    const task = createController();
    const timeout = window.setTimeout(() => task.abort(), 15_000);
    try {
      const rawData = await fetchAccounts(task.signal);
      if (epoch !== generation.current || version !== accountsVersion.current || task.signal.aborted) return;
      let data = rawData;
      if ((rawData as any).active && !rawData.accounts) {
        const act = (rawData as any).active;
        data = {
          accounts: [
            {
              id: act.id || "acc_default",
              revision: act.revision,
              username: act.username,
              upstream_user_id: act.upstream_user_id || "upstream_1",
              balance: act.balance,
              keys: act.keys || (act.selected_key ? [act.selected_key] : []),
              selected_key: act.selected_key || null,
              added_at: new Date().toISOString(),
            },
          ],
          route_account_id: act.id || "acc_default",
          candidate: rawData.candidate || null,
          operation: rawData.operation || null,
        };
      }
      const pending = pendingRef.current;
      const operation = data.operation;
      let uncertain = false;
      if (pending) {
        const matches = operation?.id === pending.id && operation.kind === pending.kind &&
          (pending.accountId === null || operation.account_id === pending.accountId);
        if (matches && operation.status !== "running") {
          if (operationAdmission.current?.pending === pending) {
            operationAdmission.current = null;
            setSending(false);
          }
          pendingRef.current = null;
          setPendingOperation(null);
          setCompletedOperation(operation);
          if (pending.kind === "refresh" && pending.accountId) {
            const accountId = pending.accountId;
            quotaBackoff.current.set(accountId, Date.now());
            const refreshed = data.accounts.find(account => account.id === accountId);
            if (operation.status === "succeeded") quotaPaused.current.delete(accountId);
            else if (operation.error?.code === "upstream_session_expired" && refreshed) {
              quotaPaused.current.set(accountId, refreshed.revision + ":" + refreshed.balance.fetched_at);
            }
            setRefreshingAccountId(previous => previous === accountId ? null : previous);
            setAccountRefreshError(previous => {
              const next = { ...previous };
              if (operation.status === "failed") next[accountId] = operation.error ? errorText(operation.error) : "余额刷新未完成，请重试";
              else delete next[accountId];
              return next;
            });
          }
          if (pending.kind === "checkin") {
            setCheckinRunError(operation.status === "failed" ? (operation.error ? errorText(operation.error) : "签到未完成，请检查记录") : "");
          }
        } else {
          pending.missingReads = matches ? 0 : pending.missingReads + 1;
          uncertain = pending.missingReads >= 15 || Date.now() - pending.startedAt >= monitorWindowMs;
        }
      } else if (operation?.status !== "running" && operation?.id === observedRunning.current) {
        setCompletedOperation(operation);
      }
      observedRunning.current = operation?.status === "running" ? operation.id : null;
      // Ignore a pre-acceptance operation/candidate while waiting for its ID.
      // Do not manufacture a running phase or a successful remote outcome.
      setState(previous => pendingRef.current && operation?.id !== pendingRef.current.id
        ? { ...data, operation: null, candidate: previous.candidate }
        : data);
      setSession("open");
      sessionRef.current = "open";
      if (quotaOpenedAt.current === null) quotaOpenedAt.current = Date.now();
      latestState.current = data;
      setLoadError(uncertain);
      monitorPaused.current = uncertain;
      setNotice(uncertain ? statusUnavailable : "");

      // Check candidate TTL
      if (data.candidate) {
        const expired = new Date(data.candidate.expires_at).getTime() <= Date.now();
        setCandidateExpired(expired);
      } else {
        setCandidateExpired(false);
      }
    } catch (error) {
      if (epoch !== generation.current || version !== accountsVersion.current)
        return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
        setNotice("");
      } else {
        setLoadError(true);
        monitorPaused.current = true;
        setNotice(pendingRef.current || task.signal.aborted ? statusUnavailable : failureText(error));
      }
    } finally {
      window.clearTimeout(timeout);
      controllers.current.delete(task);
    }
    };
    flight.promise = (async () => {
      do {
        flight.again = false;
        await read();
      } while (flight.again && epoch === generation.current && !monitorPaused.current);
    })().finally(() => {
      if (accountsFlight.current === flight) accountsFlight.current = null;
    });
    return flight.promise;
  }, [createController, lock]);

  // Load global checkin status
  const loadGlobalCheckin = useCallback(async () => {
    if (sessionRef.current !== "open") return;
    const epoch = generation.current;
    const sequence = ++checkinSequence.current;
    const task = createController();
    try {
      const data = await fetchGlobalCheckin(task.signal);
      if (epoch !== generation.current || sequence !== checkinSequence.current) return;
      setGlobalCheckin(data);
      setCheckinError("");
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted || sequence !== checkinSequence.current)
        return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
        setNotice("");
      } else {
        setCheckinError(failureText(error));
      }
    } finally {
      controllers.current.delete(task);
    }
  }, [createController, lock]);

  // Load single account checkin status & history
  const loadAccountCheckin = useCallback(
    async (accountId: string) => {
      if (sessionRef.current !== "open" || selectedAccountRef.current !== accountId) return;
      const epoch = generation.current;
      const sequence = ++accountCheckinSequence.current;
      const task = createController();
      try {
        const data = await fetchAccountCheckin(accountId, task.signal);
        if (epoch !== generation.current || sequence !== accountCheckinSequence.current || selectedAccountRef.current !== accountId) return;
        setAccountCheckin(data);
      } catch (error) {
        if (
          epoch !== generation.current ||
          task.signal.aborted ||
          sequence !== accountCheckinSequence.current || selectedAccountRef.current !== accountId
        )
          return;
        if (error instanceof ApiError && error.status === 401) {
          lock();
          setNotice("请输入本地 root 密钥以建立管理会话");
        }
      } finally {
        controllers.current.delete(task);
      }
    },
    [createController, lock]
  );

  // Load request logs
  const loadLogs = useCallback(
    async (limit = 100) => {
      if (sessionRef.current !== "open") return;
      const epoch = generation.current;
      const sequence = ++logsSequence.current;
      const task = createController();
      setLogsLoading(true);
      setLogsError(null);
      try {
        const data = await fetchRequestLogs(limit, task.signal);
        if (epoch !== generation.current || sequence !== logsSequence.current) return;
        setLogs(data.items);
      } catch (error) {
        if (epoch !== generation.current || task.signal.aborted || sequence !== logsSequence.current)
          return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
        setNotice("");
      } else {
          setLogsError("无法读取网关日志，请检查后端状态后重试");
        }
      } finally {
        controllers.current.delete(task);
        if (epoch === generation.current && sequence === logsSequence.current) {
          setLogsLoading(false);
        }
      }
    },
    [createController, lock]
  );

  // Load log settings
  const loadLogSettings = useCallback(async () => {
    if (sessionRef.current !== "open") return;
    const epoch = generation.current;
    const sequence = ++logSettingsSequence.current;
    const task = createController();
    try {
      const data = await fetchLogSettings(task.signal);
      if (epoch !== generation.current || sequence !== logSettingsSequence.current) return;
      setLogSettings(data);
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted || sequence !== logSettingsSequence.current) return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
      }
    } finally {
      controllers.current.delete(task);
    }
  }, [createController, lock]);

  // Save log settings
  const saveLogSettings = useCallback(
    async (settings: LogSettings) => {
      const epoch = generation.current;
      const sequence = ++logSettingsSequence.current;
      const task = createController();
      setSending(true);
      try {
        const data = await updateLogSettings(settings, task.signal);
        if (epoch !== generation.current || sequence !== logSettingsSequence.current) return;
        setLogSettings(data);
        return data;
      } catch (error) {
        if (epoch !== generation.current || task.signal.aborted) return;
        if (error instanceof ApiError && error.status === 401) {
          lock();
        }
        throw error;
      } finally {
        controllers.current.delete(task);
        if (epoch === generation.current) setSending(false);
      }
    },
    [createController, lock]
  );

  // Load system logs
  const loadSystemLogs = useCallback(
    async (params?: { limit?: number; level?: string; account_id?: string; operation_id?: string }) => {
      if (sessionRef.current !== "open") return;
      const epoch = generation.current;
      const sequence = ++systemLogsSequence.current;
      const task = createController();
      setSystemLogsLoading(true);
      setSystemLogsError(null);
      try {
        const data = await apiFetchSystemLogs(params, task.signal);
        if (epoch !== generation.current || sequence !== systemLogsSequence.current) return;
        setSystemLogs(data);
      } catch (error) {
        if (epoch !== generation.current || task.signal.aborted || sequence !== systemLogsSequence.current) return;
        if (error instanceof ApiError && error.status === 401) {
          lock();
        } else {
          setSystemLogsError("无法读取系统日志，请检查后端状态后重试");
        }
      } finally {
        controllers.current.delete(task);
        if (epoch === generation.current && sequence === systemLogsSequence.current) setSystemLogsLoading(false);
      }
    },
    [createController, lock]
  );

  // Clear system logs
  const clearSystemLogsAction = useCallback(async () => {
    const epoch = generation.current;
    const task = createController();
    setSending(true);
    try {
      await apiClearSystemLogs(task.signal);
      if (epoch !== generation.current) return;
      systemLogsSequence.current++;
      setSystemLogsLoading(false);
      setSystemLogs({ items: [], dropped_count: 0 });
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
      }
      throw error;
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }, [createController, lock]);

  // Clear request logs
  const clearRequestLogsAction = useCallback(async () => {
    const epoch = generation.current;
    const task = createController();
    setSending(true);
    try {
      await apiClearRequestLogs(task.signal);
      if (epoch !== generation.current) return;
      logsSequence.current++;
      setLogsLoading(false);
      setLogs([]);
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
      }
      throw error;
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }, [createController, lock]);

  // Initial account load
  useEffect(() => {
    void load();
    return abortAll;
  }, [abortAll, load]);

  // Completion-driven polling: latency above 1s cannot invalidate every read.
  useEffect(() => {
    if (session !== "open" || (!pendingOperation && state.operation?.status !== "running") || loadError) return;
    let cancelled = false;
    let timer: number;
    const poll = async () => {
      if (cancelled || monitorPaused.current) return;
      await load();
      if (!cancelled && !monitorPaused.current) timer = window.setTimeout(poll, 1000);
    };
    timer = window.setTimeout(poll, 1000);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [session, pendingOperation, state.operation?.status, loadError, load]);

  // If an accepted ID disappears, stop automatic polling with an explicit
  // unknown status. Keep admission blocked until a GET confirms its outcome.
  useEffect(() => {
    if (!pendingOperation || loadError) return;
    const timer = window.setTimeout(() => {
      if (pendingRef.current?.id !== pendingOperation.id) return;
      monitorPaused.current = true;
      setLoadError(true);
      setNotice(statusUnavailable);
    }, Math.max(0, pendingOperation.startedAt + monitorWindowMs - Date.now()));
    return () => window.clearTimeout(timer);
  }, [pendingOperation, loadError]);

  // Refresh global checkin, logs, and settings when session opens
  useEffect(() => {
    if (session !== "open") return;
    void loadGlobalCheckin();
    void loadLogs();
    void loadLogSettings();
    void loadSystemLogs();
  }, [session, loadGlobalCheckin, loadLogs, loadLogSettings, loadSystemLogs]);

  // Also handle operations that finish before the first status GET arrives.
  useEffect(() => {
    if (completedOperation && session === "open") {
      void loadGlobalCheckin();
      void loadLogs();
      // Completion is recorded once by the current-session account reader.
      // Log failures stay in systemLogsError, separate from the login outcome.
      if (completedOperation.kind === "login") void loadSystemLogs();
      if (selectedAccountRef.current) {
        void loadAccountCheckin(selectedAccountRef.current);
      }
    }
  }, [completedOperation, session, loadGlobalCheckin, loadLogs, loadSystemLogs, loadAccountCheckin]);

  // Check candidate expiration every second
  useEffect(() => {
    if (!state.candidate) {
      setCandidateExpired(false);
      return;
    }
    const checkExpiry = () => {
      if (!state.candidate) return;
      const expired = new Date(state.candidate.expires_at).getTime() <= Date.now();
      setCandidateExpired(expired);
    };
    checkExpiry();
    const timer = window.setInterval(checkExpiry, 1000);
    return () => window.clearInterval(timer);
  }, [state.candidate]);

  // Refresh checkin when tab becomes visible
  useEffect(() => {
    if (session !== "open") return;
    const onVisibility = () => {
      if (document.visibilityState === "visible") {
        quotaReconciling.current = true;
        const epoch = generation.current;
        void load().finally(() => {
          if (epoch !== generation.current) return;
          quotaReconciling.current = false;
          setQuotaWake(value => value + 1);
        });
        void loadGlobalCheckin();
        void loadLogs();
        if (selectedAccountId) {
          void loadAccountCheckin(selectedAccountId);
        }
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [session, selectedAccountId, load, loadGlobalCheckin, loadLogs, loadAccountCheckin]);

  // Bounded timer for next scheduled run or 08:00 CST boundary
  useEffect(() => {
    if (session !== "open") return;
    const now = Date.now();
    let delay: number | null = null;

    if (globalCheckin?.next_run_at) {
      const nextRunTime = new Date(globalCheckin.next_run_at).getTime();
      const diff = nextRunTime - now;
      if (diff > 0 && diff <= 86400000) {
        delay = diff + 1000;
      }
    }

    const nowDate = new Date(now);
    const nextUtcMidnight = new Date(
      Date.UTC(nowDate.getUTCFullYear(), nowDate.getUTCMonth(), nowDate.getUTCDate() + 1, 0, 0, 1)
    );
    const diffTo08 = nextUtcMidnight.getTime() - now;
    if (diffTo08 > 0 && diffTo08 <= 86400000) {
      delay = delay === null ? diffTo08 : Math.min(delay, diffTo08);
    }

    if (delay === null) return;
    const timer = window.setTimeout(() => {
      void loadGlobalCheckin();
    }, delay);
    return () => window.clearTimeout(timer);
  }, [session, globalCheckin?.next_run_at, loadGlobalCheckin]);

  // Unlock management session
  async function unlock(root: string) {
    const epoch = generation.current;
    const task = createController();
    setSending(true);
    setNotice("");
    try {
      const pending = adminLogin(root, task.signal);
      root = "";
      await pending;
      if (epoch !== generation.current) return;
      await load();
      await loadGlobalCheckin();
      await loadLogs();
    } catch (error) {
      if (epoch === generation.current && !task.signal.aborted) {
        setNotice(
          error instanceof ApiError && error.status === 401
            ? "密钥不正确，请重试"
            : failureText(error)
        );
      }
    } finally {
      root = "";
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }

  // Logout admin session
  async function logout() {
    lock();
    setNotice("已清除本页账号信息，正在注销管理会话");
    const epoch = generation.current;
    const task = createController();
    setSending(true);
    try {
      await adminLogout(task.signal);
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

  // Add account by logging in
  async function addAccount(credentials: { username: string; password: string }) {
    const owner = reserveOperation();
    if (!owner) { credentials.password = ""; return; }
    const epoch = generation.current;
    const task = createController();
    setNotice("");
    try {
      const pending = loginAccount(credentials.username, credentials.password, task.signal);
      credentials.password = "";
      const accepted = await pending;
      if (epoch !== generation.current) return;
      trackOperation(owner, accepted.operation_id, "login");
      await load();
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.status === 401) {
        throw new Error("管理请求未通过鉴权，请重试或重新建立管理会话");
      }
      throw new Error(failureText(error));
    } finally {
      credentials.password = "";
      controllers.current.delete(task);
      finishSubmission(owner);
    }
  }

  // Save candidate as account
  async function saveCandidate(candidateId: string, keyId?: string | null) {
    const owner = reserveOperation();
    if (!owner) return;
    const epoch = generation.current;
    const task = createController();
    setNotice("");
    try {
      const accepted = await saveAccount(candidateId, keyId ?? null, task.signal);
      if (epoch !== generation.current) return;
      trackOperation(owner, accepted.operation_id, "save");
      await load();
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError) {
        if (error.status === 401) {
          throw new Error("管理请求未通过鉴权，请重新建立管理会话");
        }
        if (error.detail.code === "candidate_expired") {
          setCandidateExpired(true);
        }
      }
      throw new Error(failureText(error));
    } finally {
      controllers.current.delete(task);
      finishSubmission(owner);
    }
  }

  // Refresh account balance
  async function refreshAccountBalance(accountId: string) {
    const owner = reserveOperation();
    if (!owner) return;
    quotaRound.current.seen.add(accountId);
    const epoch = generation.current;
    const task = createController();
    let accepted = false;
    setRefreshingAccountId(accountId);
    setAccountRefreshError(prev => {
      const next = { ...prev };
      delete next[accountId];
      return next;
    });
    setNotice("");
    try {
      const response = await apiRefreshAccount(accountId, task.signal);
      if (epoch !== generation.current) return;
      trackOperation(owner, response.operation_id, "refresh", accountId);
      accepted = true;
      await load();
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.status === 401) {
        lock();
        return;
      }
      quotaBackoff.current.set(accountId, Date.now());
      if (error instanceof ApiError && error.detail.code === "upstream_session_expired") {
        const account = latestState.current.accounts.find(value => value.id === accountId);
        if (account) quotaPaused.current.set(accountId, account.revision + ":" + account.balance.fetched_at);
      }
      const errText = failureText(error);
      setAccountRefreshError(prev => ({ ...prev, [accountId]: errText }));
      if (!(error instanceof ApiError)) {
        // A lost acceptance response does not prove that the server did no work.
        monitorPaused.current = true;
        setLoadError(true);
      }
    } finally {
      controllers.current.delete(task);
      if (operationAdmission.current === owner) {
        if (!accepted) {
          setRefreshingAccountId(previous => previous === accountId ? null : previous);
        }
        finishSubmission(owner);
      }
    }
  }

  // Page-local scheduling only; no cross-tab lock and no server-side cancellation.
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState !== "visible") quotaReconciling.current = true;
      else if (sessionRef.current !== "open") quotaReconciling.current = false;
      setQuotaVisible(document.visibilityState === "visible");
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, []);

  useEffect(() => {
    if (!quotaAutoRefreshEnabled || session !== "open" || !quotaVisible || loadError
        || sending || pendingOperation || state.operation?.status === "running"
        || operationAdmission.current || quotaReconciling.current || quotaOpenedAt.current === null) return;
    const interval = quotaRefreshIntervalMinutes * 60_000;
    const now = Date.now();
    // One admission per account per interval-sized round; never replay missed
    // rounds after inactivity. Oldest due accounts precede recently attempted ones.
    if (now >= quotaRound.current.startedAt + interval) {
      quotaRound.current = { startedAt: now, seen: new Set() };
    }
    const due = state.accounts.slice(0, 64).flatMap(account => {
      const fingerprint = account.revision + ":" + account.balance.fetched_at;
      if (quotaPaused.current.get(account.id) === fingerprint) return [];
      quotaPaused.current.delete(account.id);
      const fetched = Date.parse(account.balance.fetched_at);
      const time = Math.max(quotaOpenedAt.current! + interval,
        Number.isFinite(fetched) ? fetched + interval : quotaOpenedAt.current! + interval,
        (quotaBackoff.current.get(account.id) ?? -Infinity) + interval,
        quotaRound.current.seen.has(account.id) ? quotaRound.current.startedAt + interval : 0);
      return [{ id: account.id, time }];
    }).sort((a, b) => a.time - b.time || a.id.localeCompare(b.id));
    if (!due.length) return;
    const epoch = generation.current;
    const timer = window.setTimeout(() => {
      if (epoch !== generation.current || sessionRef.current !== "open"
          || document.visibilityState !== "visible" || quotaReconciling.current) return;
      if (Date.now() < due[0].time) {
        setQuotaWake(value => value + 1);
        return;
      }
      void refreshAccountBalance(due[0].id);
    }, Math.min(interval, Math.max(0, due[0].time - now)));
    return () => window.clearTimeout(timer);
  }, [quotaAutoRefreshEnabled, quotaRefreshIntervalMinutes, quotaVisible, quotaWake, session,
    loadError, sending, pendingOperation, state]);

  // Select account key
  async function selectAccountKey(accountId: string, keyId: string) {
    const targetAccount = state.accounts.find(a => a.id === accountId);
    if (!targetAccount) return;
    const owner = reserveOperation();
    if (!owner) return;
    const epoch = generation.current;
    const task = createController();
    setNotice("");
    try {
      const accepted = await apiSelectAccountKey(accountId, targetAccount.revision, keyId, task.signal);
      if (epoch !== generation.current) return;
      trackOperation(owner, accepted.operation_id, "select_key", accountId);
      await load();
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError && error.status === 401) {
        setNotice("管理请求未通过鉴权，请重试或重新建立管理会话");
        return;
      }
      setNotice(failureText(error));
    } finally {
      controllers.current.delete(task);
      finishSubmission(owner);
    }
  }

  // Route gateway to an account
  async function routeAccount(accountId: string) {
    const targetAccount = state.accounts.find(a => a.id === accountId);
    if (!targetAccount) return;
    if (!targetAccount.selected_key) {
      setNotice("该账号未关联有效密钥，无法作为网关路由出口");
      return;
    }
    const epoch = generation.current;
    const task = createController();
    setSending(true);
    setPendingRouteAccountId(accountId);
    setNotice("");
    try {
      const res = await apiRouteAccount(accountId, targetAccount.revision, task.signal);
      if (epoch !== generation.current) return;
      accountsVersion.current++;
      setState(res);
      setPendingRouteAccountId(null);
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      setPendingRouteAccountId(null);
      if (error instanceof ApiError && error.status === 401) {
        lock();
        setNotice("管理请求未通过鉴权，请重新建立管理会话");
        return;
      }
      setNotice(failureText(error));
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setSending(false);
    }
  }

  // Reveal account key (root must be header only, 401 must not logout admin session)
  async function revealKey(accountId: string, root: string, externalSignal?: AbortSignal): Promise<string> {
    const targetAccount = state.accounts.find(a => a.id === accountId);
    if (!targetAccount) throw new Error("找不到指定账号");
    const accountRevision = targetAccount.revision;
    const epoch = generation.current;
    const task = createController();
    if (externalSignal) {
      if (externalSignal.aborted) {
        task.abort();
      } else {
        externalSignal.addEventListener("abort", () => task.abort(), { once: true });
      }
    }
    try {
      const result = await revealAccountKey(accountId, accountRevision, root, task.signal);
      root = "";
      if (epoch !== generation.current || task.signal.aborted) {
        throw new Error("请求已中止");
      }
      // Late response check: account ID and revision must match
      if (result.account_id !== accountId || result.revision !== accountRevision) {
        throw new Error("账号版本已变化，已丢弃返回的密钥，请重新读取账号状态");
      }
      return result.key;
    } catch (error) {
      root = "";
      if (error instanceof ApiError && error.status === 401) {
        // Modal error only; do not logout admin session!
        throw new Error("本地 root 密钥未通过验证，请重试");
      }
      throw error instanceof Error ? error : new Error("查看密钥失败");
    } finally {
      root = "";
      controllers.current.delete(task);
    }
  }

  // Run account manual checkin
  async function runAccountCheckin(accountId: string, options: { confirmRetry: boolean }) {
    const owner = reserveOperation();
    if (!owner) return;
    const epoch = generation.current;
    const task = createController();
    setCheckinRunError("");
    try {
      const res = await apiRunAccountCheckin(accountId, options.confirmRetry, task.signal);
      if (epoch !== generation.current) return;
      if (res.already_recorded) {
        await loadGlobalCheckin();
        await loadAccountCheckin(accountId);
      } else {
        trackOperation(owner, res.operation_id ?? "", "checkin", accountId);
        await load();
        void loadGlobalCheckin();
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
          setCheckinRunError(
            "当前周期签到结果处于未确认状态，重试可能导致重复提交，请确认后重试"
          );
          return;
        }
      }
      setCheckinRunError(failureText(error));
      throw error;
    } finally {
      controllers.current.delete(task);
      finishSubmission(owner);
    }
  }

  // Update global checkin settings
  async function updateGlobalCheckin(config: {
    enabled: boolean;
    startTime: string;
    intervalMinutes: number;
  }) {
    const epoch = generation.current;
    const task = createController();
    setCheckinSaving(true);
    setCheckinRunError("");
    try {
      const data = await updateGlobalCheckinSettings(
        {
          enabled: config.enabled,
          time: config.startTime,
          interval_minutes: config.intervalMinutes,
        },
        task.signal
      );
      if (epoch !== generation.current) return;
      setGlobalCheckin(data);
      setCheckinError("");
    } catch (error) {
      if (epoch !== generation.current || task.signal.aborted) return;
      if (error instanceof ApiError) {
        if (error.status === 401) {
          lock();
          setNotice("请输入本地 root 密钥以建立管理会话");
          return;
        }
        if (error.status === 422 || error.detail.code === "schedule_overflow") {
          const msg = "签到排期跨天溢出，请缩短间隔或调整起始时间";
          setCheckinRunError(msg);
          throw new Error(msg);
        }
      }
      setCheckinRunError(failureText(error));
      throw error;
    } finally {
      controllers.current.delete(task);
      if (epoch === generation.current) setCheckinSaving(false);
    }
  }

  // Select account for detail viewing (does NOT route!)
  function selectDetailAccount(accountId: string | null) {
    selectedAccountRef.current = accountId;
    accountCheckinSequence.current++;
    setSelectedAccountId(accountId);
    setAccountCheckin(null);
    if (accountId) {
      void loadAccountCheckin(accountId);
    } else {
      setAccountCheckin(null);
    }
  }

  return {
    session,
    state,
    pendingRouteAccountId,
    notice,
    sending,
    loadError,
    candidateExpired,
    busy: sending || pendingOperation !== null || state.operation?.status === "running",
    globalCheckin,
    accountCheckin,
    selectedAccountId,
    checkinError,
    checkinRunError,
    checkinSaving,
    logs,
    logsLoading,
    logsError,
    refreshingAccountId,
    accountRefreshError,
    quotaAutoRefreshEnabled,
    quotaRefreshIntervalMinutes,
    setQuotaAutoRefreshEnabled,
    setQuotaRefreshIntervalMinutes,
    logSettings,
    systemLogs,
    systemLogsLoading,
    systemLogsError,
    load,
    loadGlobalCheckin,
    loadAccountCheckin,
    loadLogs,
    loadLogSettings,
    saveLogSettings,
    loadSystemLogs,
    clearSystemLogs: clearSystemLogsAction,
    clearRequestLogs: clearRequestLogsAction,
    unlock,
    logout,
    addAccount,
    saveCandidate,
    refreshAccount: refreshAccountBalance,
    selectAccountKey,
    routeAccount,
    revealKey,
    runAccountCheckin,
    updateGlobalCheckin,
    selectDetailAccount,
  };
}
