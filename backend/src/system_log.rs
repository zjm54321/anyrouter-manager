//! Bounded typed metadata only. No messages, arbitrary maps, body or dependency logs.
pub use crate::diagnostics::LoginDiagnostics as SafeLoginDiagnostics;
use crate::log_settings::{LogLevel, SharedLogSettings, atomic_private_write, read_private};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

pub const SYSTEM_LOG_ERROR: &str = "system_log_failed";
pub const SYSTEM_LOG_INVALID: &str = "system_log_invalid";
pub const MAX_EVENT_BYTES: usize = 4096;
pub const MAX_ITEMS: usize = 10_000;
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const QUEUE_CAPACITY: usize = 256;
pub const MAX_PENDING_BYTES: usize = 1024 * 1024;
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    AccountRefresh,
    AccountSave,
    KeySelected,
    AppStartup,
    AppShutdown,
    TaskStart,
    TaskFinish,
    HttpLogin,
    BrowserWait,
    BrowserSpawn,
    HelperResult,
    Cleanup,
    Timeout,
    CheckinStart,
    CheckinFinish,
    StorageRead,
    StorageWrite,
    RouteStart,
    RouteFinish,
    LoginStart,
    LoginFinish,
    SettingsUpdated,
    LogCleared,
    OperationCancelled,
    AccountSelected,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventStage {
    SelfAccount,
    Tokens,
    Key,
    CheckinPreflight,
    Admission,
    HttpLogin,
    BrowserWait,
    BrowserSpawn,
    Navigation,
    FormWait,
    Submit,
    ProfileWait,
    HelperResult,
    Cleanup,
    Checkin,
    Storage,
    Route,
    Shutdown,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventReason {
    KnownChallenge,
    RedirectRejected,
    HttpRejected,
    BodyEncoding,
    BodyDecode,
    BodyLimit,
    JsonInvalid,
    JsonRejected,
    SchemaInvalid,
    IdentityMismatch,
    QuotaInvalid,
    SessionExpired,
    PreflightFailed,
    OutcomeUnknown,
    AlreadyDone,
    UnsupportedResponse,
    DeadlineExpired,
    IoFailure,
    NetworkFailure,
    InvalidState,
    InvalidMetadata,
    AuthenticationRejected,
    HelperFailed,
    Cancelled,
    QueueFull,
    StorageUnavailable,
    NotReady,
    Success,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "EventWire")]
pub struct SystemEvent {
    pub id: String,
    pub timestamp: String,
    pub level: LogLevel,
    pub event: EventKind,
    pub operation_id: Option<String>,
    pub account_id: Option<String>,
    pub stage: Option<EventStage>,
    pub elapsed_ms: Option<u64>,
    pub http_status: Option<u16>,
    pub reason: Option<EventReason>,
    pub diagnostics: Option<SafeLoginDiagnostics>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventWire {
    id: String,
    timestamp: String,
    level: LogLevel,
    event: EventKind,
    #[serde(deserialize_with = "Option::deserialize")]
    operation_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    account_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    stage: Option<EventStage>,
    #[serde(deserialize_with = "Option::deserialize")]
    elapsed_ms: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    http_status: Option<u16>,
    #[serde(deserialize_with = "Option::deserialize")]
    reason: Option<EventReason>,
    #[serde(deserialize_with = "Option::deserialize")]
    diagnostics: Option<SafeLoginDiagnostics>,
}
impl TryFrom<EventWire> for SystemEvent {
    type Error = &'static str;
    fn try_from(value: EventWire) -> Result<Self, Self::Error> {
        let event = Self {
            id: value.id,
            timestamp: value.timestamp,
            level: value.level,
            event: value.event,
            operation_id: value.operation_id,
            account_id: value.account_id,
            stage: value.stage,
            elapsed_ms: value.elapsed_ms,
            http_status: value.http_status,
            reason: value.reason,
            diagnostics: value.diagnostics,
        };
        event.validate()?;
        Ok(event)
    }
}
fn valid_id(value: &str) -> bool {
    value.len() == 36
        && uuid::Uuid::parse_str(value)
            .is_ok_and(|id| id.hyphenated().to_string().eq_ignore_ascii_case(value))
}
impl SystemEvent {
    pub fn new(level: LogLevel, event: EventKind) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
            level,
            event,
            operation_id: None,
            account_id: None,
            stage: None,
            elapsed_ms: None,
            http_status: None,
            reason: None,
            diagnostics: None,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_id(&self.id)
            || self.timestamp.len() > 40
            || !DateTime::parse_from_rfc3339(&self.timestamp)
                .is_ok_and(|t| t.offset().local_minus_utc() == 0)
            || !(self.timestamp.ends_with('Z') || self.timestamp.ends_with("+00:00"))
            || !self.operation_id.as_deref().is_none_or(valid_id)
            || !self.account_id.as_deref().is_none_or(valid_id)
            || !self.http_status.is_none_or(|s| (100..=599).contains(&s))
        {
            return Err(SYSTEM_LOG_INVALID);
        }
        if serde_json::to_vec(self)
            .map_err(|_| SYSTEM_LOG_INVALID)?
            .len()
            > MAX_EVENT_BYTES
        {
            return Err(SYSTEM_LOG_INVALID);
        }
        Ok(())
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(SYSTEM_LOG_INVALID);
        }
        serde_json::from_slice(bytes).map_err(|_| SYSTEM_LOG_INVALID)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u8,
    items: Vec<SystemEvent>,
    dropped_count: u64,
}
impl Default for Store {
    fn default() -> Self {
        Self {
            version: 1,
            items: Vec::new(),
            dropped_count: 0,
        }
    }
}
impl Store {
    fn add_dropped(&mut self, count: u64) {
        self.dropped_count = self.dropped_count.saturating_add(count);
    }
    fn prune(&mut self, now: DateTime<Utc>, days: u16) {
        let cutoff = now - chrono::Duration::days(i64::from(days));
        let before = self.items.len();
        self.items
            .retain(|e| DateTime::parse_from_rfc3339(&e.timestamp).is_ok_and(|t| t >= cutoff));
        self.add_dropped((before - self.items.len()) as u64);
    }
    fn bound(&mut self) -> Result<(), &'static str> {
        self.bound_limits(MAX_ITEMS, MAX_FILE_BYTES)
    }
    fn bound_limits(&mut self, max_items: usize, max_bytes: usize) -> Result<(), &'static str> {
        self.items.sort_by(|a, b| {
            DateTime::parse_from_rfc3339(&a.timestamp)
                .ok()
                .cmp(&DateTime::parse_from_rfc3339(&b.timestamp).ok())
                .then_with(|| a.id.cmp(&b.id))
        });
        // Exact envelope + item lengths; avoid repeated whole-file serialization.
        let mut bytes = serde_json::to_vec(self)
            .map_err(|_| SYSTEM_LOG_ERROR)?
            .len();
        let mut remove = 0;
        while self.items.len() - remove > max_items || bytes > max_bytes {
            let Some(event) = self.items.get(remove) else {
                return Err(SYSTEM_LOG_ERROR);
            };
            bytes = bytes.saturating_sub(
                serde_json::to_vec(event)
                    .map_err(|_| SYSTEM_LOG_ERROR)?
                    .len()
                    + 1,
            );
            remove += 1;
        }
        self.items.drain(..remove);
        self.add_dropped(remove as u64);
        // dropped_count may gain digits; trim the very small envelope delta too.
        while serde_json::to_vec(self)
            .map_err(|_| SYSTEM_LOG_ERROR)?
            .len()
            > max_bytes
        {
            if self.items.is_empty() {
                return Err(SYSTEM_LOG_ERROR);
            }
            self.items.remove(0);
            self.add_dropped(1);
        }
        Ok(())
    }
}
fn load(path: &Path) -> Result<Store, &'static str> {
    let Some(bytes) = read_private(path, MAX_FILE_BYTES).map_err(|_| SYSTEM_LOG_ERROR)? else {
        return Ok(Store::default());
    };
    let store: Store = serde_json::from_slice(&bytes).map_err(|_| SYSTEM_LOG_INVALID)?;
    if store.version != 1 || store.items.len() > MAX_ITEMS {
        return Err(SYSTEM_LOG_INVALID);
    }
    Ok(store)
}
fn persist(path: &Path, store: &Store) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(store).map_err(|_| SYSTEM_LOG_ERROR)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(SYSTEM_LOG_ERROR);
    }
    atomic_private_write(path, &bytes).map_err(|_| SYSTEM_LOG_ERROR)
}

#[derive(Clone, Serialize)]
pub struct SystemLogPage {
    pub items: Vec<SystemEvent>,
    pub dropped_count: u64,
}
#[derive(Default, Clone)]
pub struct SystemLogQuery {
    pub limit: Option<usize>,
    pub level: Option<LogLevel>,
    pub account_id: Option<String>,
    pub operation_id: Option<String>,
}
/// Opaque admission token for operations started before clear. Capture at start,
/// and enqueue on completion to prevent older operations from repopulating logs.
#[derive(Clone, Copy)]
pub struct SystemLogGeneration(u64);
enum Message {
    Append(SystemEvent, usize, SystemLogGeneration),
    Barrier(oneshot::Sender<Result<(), &'static str>>, Control),
}
#[derive(Clone, Copy)]
enum Control {
    Flush,
    Clear,
    Shutdown,
}
struct Pending {
    closed: bool,
    bytes: usize,
}
struct Shared {
    store: Mutex<Store>,
    dropped: AtomicU64,
    failed: AtomicBool,
    generation: AtomicU64,
    pending: Mutex<Pending>,
}
#[derive(Clone)]
pub struct SystemLogSink {
    sender: mpsc::Sender<Message>,
    shared: Arc<Shared>,
    settings: SharedLogSettings,
}
impl SystemLogSink {
    pub async fn open(path: PathBuf, settings: SharedLogSettings) -> Result<Self, &'static str> {
        let source = path.clone();
        let days = settings.snapshot().system_retention_days;
        let store = tokio::task::spawn_blocking(move || {
            let mut store = load(&source)?;
            store.prune(Utc::now(), days);
            store.bound()?;
            persist(&source, &store)?;
            Ok::<_, &'static str>(store)
        })
        .await
        .map_err(|_| SYSTEM_LOG_ERROR)??;
        let shared = Arc::new(Shared {
            store: Mutex::new(store),
            dropped: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            pending: Mutex::new(Pending {
                closed: false,
                bytes: 0,
            }),
        });
        let (sender, mut receiver) = mpsc::channel(QUEUE_CAPACITY);
        let worker = shared.clone();
        let preferences = settings.clone();
        let mut changed = settings.subscribe();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            let mut buffered = None;
            loop {
                let message = if let Some(message) = buffered.take() {
                    Some(message)
                } else {
                    tokio::select! {
                        item = receiver.recv() => { match item { Some(item) => Some(item), None => break } },
                        _ = interval.tick() => None,
                        result = changed.changed() => { if result.is_err() { break; } None },
                    }
                };
                let (entry, size, barrier) = match message {
                    Some(Message::Append(entry, size, generation)) => {
                        (Some((entry, generation)), size, None)
                    }
                    Some(Message::Barrier(reply, control)) => (None, 0, Some((reply, control))),
                    None => (None, 0, None),
                };
                // Batch only consecutive appends: never cross a clear/flush
                // barrier. This bounds fsync overhead during a full-queue drain.
                let mut entries = entry.into_iter().collect::<Vec<_>>();
                let mut size = size;
                if !entries.is_empty() {
                    while entries.len() < 64 {
                        match receiver.try_recv() {
                            Ok(Message::Append(entry, bytes, generation)) => {
                                entries.push((entry, generation));
                                size += bytes;
                            }
                            Ok(message) => {
                                buffered = Some(message);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                }
                let control = barrier.as_ref().map(|(_, c)| *c);
                let state = worker.clone();
                let target = path.clone();
                let days = preferences.snapshot().system_retention_days;
                let result = tokio::task::spawn_blocking(move || {
                    if matches!(control, Some(Control::Clear)) {
                        let empty = Store::default();
                        persist(&target, &empty)?;
                        // Queue admission and clear publication share this short gate.
                        let _pending = state.pending.lock().map_err(|_| SYSTEM_LOG_ERROR)?;
                        *state.store.lock().map_err(|_| SYSTEM_LOG_ERROR)? = empty;
                        state.dropped.store(0, Ordering::Release);
                        state.generation.fetch_add(1, Ordering::AcqRel);
                        return Ok(());
                    }
                    let mut store = state.store.lock().map_err(|_| SYSTEM_LOG_ERROR)?.clone();
                    store.add_dropped(state.dropped.swap(0, Ordering::AcqRel));
                    for (entry, generation) in entries {
                        if generation.0 == state.generation.load(Ordering::Acquire) {
                            store.items.push(entry);
                        } else {
                            store.add_dropped(1);
                        }
                    }
                    store.prune(Utc::now(), days);
                    store.bound()?;
                    // Readers do not wait on filesystem IO. Disk failure is flagged,
                    // but bounded in-memory metadata remains available for diagnosis.
                    *state.store.lock().map_err(|_| SYSTEM_LOG_ERROR)? = store.clone();
                    persist(&target, &store)
                })
                .await
                .unwrap_or(Err(SYSTEM_LOG_ERROR));
                if let Ok(mut pending) = worker.pending.lock() {
                    pending.bytes = pending.bytes.saturating_sub(size);
                }
                if result.is_err() {
                    worker.failed.store(true, Ordering::Release);
                }
                if let Some((reply, control)) = barrier {
                    // A successful clear is allowed to recover a previous disk failure.
                    if matches!(control, Control::Clear) && result.is_ok() {
                        worker.failed.store(false, Ordering::Release);
                    }
                    let _ = reply.send(if worker.failed.load(Ordering::Acquire) {
                        Err(SYSTEM_LOG_ERROR)
                    } else {
                        result
                    });
                    if matches!(control, Control::Shutdown) {
                        break;
                    }
                }
            }
        });
        Ok(Self {
            sender,
            shared,
            settings,
        })
    }
    pub fn capture_generation(&self) -> SystemLogGeneration {
        SystemLogGeneration(self.shared.generation.load(Ordering::Acquire))
    }
    pub fn emit(&self, event: SystemEvent) -> bool {
        self.try_enqueue(event)
    }
    pub fn try_enqueue(&self, event: SystemEvent) -> bool {
        self.try_enqueue_captured(event, self.capture_generation())
    }
    pub fn try_enqueue_captured(
        &self,
        event: SystemEvent,
        generation: SystemLogGeneration,
    ) -> bool {
        if event.validate().is_err() {
            self.drop_one();
            return false;
        }
        if !self.settings.snapshot().level.allows(event.level) {
            return false;
        }
        let Ok(bytes) = serde_json::to_vec(&event) else {
            self.drop_one();
            return false;
        };
        let size = bytes.len();
        let Ok(mut pending) = self.shared.pending.lock() else {
            self.drop_one();
            return false;
        };
        if pending.closed
            || size > MAX_PENDING_BYTES.saturating_sub(pending.bytes)
            || self
                .sender
                .try_send(Message::Append(event, size, generation))
                .is_err()
        {
            self.drop_one();
            return false;
        }
        pending.bytes += size;
        true
    }
    fn drop_one(&self) {
        let _ = self
            .shared
            .dropped
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                Some(n.saturating_add(1))
            });
    }
    pub fn page(&self, query: SystemLogQuery) -> Result<SystemLogPage, &'static str> {
        if query.limit.is_some_and(|n| !(1..=1000).contains(&n))
            || !query.account_id.as_deref().is_none_or(valid_id)
            || !query.operation_id.as_deref().is_none_or(valid_id)
        {
            return Err(SYSTEM_LOG_INVALID);
        }
        let store = self.shared.store.lock().map_err(|_| SYSTEM_LOG_ERROR)?;
        Ok(SystemLogPage {
            items: store
                .items
                .iter()
                .rev()
                .filter(|e| query.level.is_none_or(|l| l.allows(e.level)))
                .filter(|e| {
                    query
                        .account_id
                        .as_ref()
                        .is_none_or(|id| e.account_id.as_ref() == Some(id))
                })
                .filter(|e| {
                    query
                        .operation_id
                        .as_ref()
                        .is_none_or(|id| e.operation_id.as_ref() == Some(id))
                })
                .take(query.limit.unwrap_or(100))
                .cloned()
                .collect(),
            dropped_count: store
                .dropped_count
                .saturating_add(self.shared.dropped.load(Ordering::Acquire)),
        })
    }
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }
    /// Route adapter maps this fixed failure to HTTP 503; never serialize IO text.
    pub async fn clear(&self) -> Result<(), &'static str> {
        self.barrier(Control::Clear).await
    }
    pub async fn flush(&self) -> Result<(), &'static str> {
        self.barrier(Control::Flush).await
    }
    pub async fn shutdown(&self) -> Result<(), &'static str> {
        self.barrier(Control::Shutdown).await
    }
    async fn barrier(&self, control: Control) -> Result<(), &'static str> {
        let operation = async {
            let permit = self.sender.reserve().await.map_err(|_| SYSTEM_LOG_ERROR)?;
            let (reply, receive) = oneshot::channel();
            {
                let mut pending = self.shared.pending.lock().map_err(|_| SYSTEM_LOG_ERROR)?;
                if pending.closed {
                    return Err(SYSTEM_LOG_ERROR);
                }
                if matches!(control, Control::Shutdown) {
                    pending.closed = true;
                }
                permit.send(Message::Barrier(reply, control));
            }
            receive.await.map_err(|_| SYSTEM_LOG_ERROR)?
        };
        // Clear reports only a definitive writer outcome, never a timeout
        // falsely claiming that an already-submitted atomic rename rolled back.
        // HTTP callers that cancel clear must treat its outcome as unknown.
        if matches!(control, Control::Clear) {
            operation.await
        } else {
            tokio::time::timeout(CONTROL_TIMEOUT, operation)
                .await
                .map_err(|_| SYSTEM_LOG_ERROR)?
        }
    }
}

/// Only this target and a single string-valued `event_json` field are accepted.
/// Debug/display visitors are deliberately rejected, not converted to strings.
#[derive(Clone)]
pub struct SystemLogLayer {
    sink: SystemLogSink,
}
impl SystemLogLayer {
    pub fn new(sink: SystemLogSink) -> Self {
        Self { sink }
    }
}
struct TypedVisitor {
    event: Option<SystemEvent>,
    invalid: bool,
}
impl tracing::field::Visit for TypedVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() != "event_json" || self.event.is_some() {
            self.invalid = true;
            return;
        }
        match SystemEvent::from_json(value.as_bytes()) {
            Ok(event) => self.event = Some(event),
            Err(_) => self.invalid = true,
        }
    }
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {
        self.invalid = true;
    }
}
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SystemLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let metadata = event.metadata();
        if metadata.target() != "anyrouter.system" {
            return;
        }
        if metadata.fields().len() != 1 {
            self.sink.drop_one();
            return;
        }
        let mut visitor = TypedVisitor {
            event: None,
            invalid: false,
        };
        event.record(&mut visitor);
        if !visitor.invalid
            && let Some(typed) = visitor.event
        {
            let level = match *metadata.level() {
                tracing::Level::ERROR => LogLevel::Error,
                tracing::Level::WARN => LogLevel::Warn,
                tracing::Level::INFO => LogLevel::Info,
                tracing::Level::DEBUG => LogLevel::Debug,
                tracing::Level::TRACE => LogLevel::Trace,
            };
            if typed.level == level {
                self.sink.emit(typed);
                return;
            }
        }
        self.sink.drop_one();
    }
}
/// Optional typed tracing hook. Install only SystemLogLayer (no formatter for
/// this target); the caller may also bypass tracing with sink.emit(event).
pub fn emit_tracing(event: &SystemEvent) -> Result<(), &'static str> {
    event.validate()?;
    let json = serde_json::to_string(event).map_err(|_| SYSTEM_LOG_INVALID)?;
    match event.level {
        LogLevel::Error => {
            tracing::event!(target: "anyrouter.system", tracing::Level::ERROR, event_json = json.as_str())
        }
        LogLevel::Warn => {
            tracing::event!(target: "anyrouter.system", tracing::Level::WARN, event_json = json.as_str())
        }
        LogLevel::Info => {
            tracing::event!(target: "anyrouter.system", tracing::Level::INFO, event_json = json.as_str())
        }
        LogLevel::Debug => {
            tracing::event!(target: "anyrouter.system", tracing::Level::DEBUG, event_json = json.as_str())
        }
        LogLevel::Trace => {
            tracing::event!(target: "anyrouter.system", tracing::Level::TRACE, event_json = json.as_str())
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_settings::LogSettings;
    use std::{fs, os::unix::fs::PermissionsExt};
    async fn fixture() -> (tempfile::TempDir, PathBuf, SharedLogSettings, SystemLogSink) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/system_log.json");
        let settings = SharedLogSettings::open(tmp.path().join("private/log_settings.json"))
            .await
            .unwrap();
        let sink = SystemLogSink::open(path.clone(), settings.clone())
            .await
            .unwrap();
        (tmp, path, settings, sink)
    }
    fn event() -> SystemEvent {
        SystemEvent::new(LogLevel::Info, EventKind::HttpLogin)
    }
    #[test]
    fn strict_schema_ids_status_elapsed_and_no_secret_fields() {
        let value = serde_json::to_value(event()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 11);
        for (key, v) in [
            ("password", serde_json::json!("FAKE-secret")),
            ("cookie", serde_json::json!("FAKE")),
            ("event", serde_json::json!("FAKE-url?key=secret")),
            ("stage", serde_json::json!("raw")),
            ("reason", serde_json::json!("raw")),
            ("level", serde_json::json!("unknown")),
            ("id", serde_json::json!("old-op-id")),
            ("account_id", serde_json::json!("username")),
            ("operation_id", serde_json::json!("op-id")),
            ("http_status", serde_json::json!(99)),
            ("http_status", serde_json::json!(600)),
            ("elapsed_ms", serde_json::json!(-1)),
            ("elapsed_ms", serde_json::json!(1.0)),
            ("timestamp", serde_json::json!("2026-10-01T00:00:00+01:00")),
        ] {
            let mut invalid = value.clone();
            invalid[key] = v;
            assert!(matches!(
                SystemEvent::from_json(&serde_json::to_vec(&invalid).unwrap()),
                Err(SYSTEM_LOG_INVALID)
            ));
        }
        for key in value.as_object().unwrap().keys() {
            let mut invalid = value.clone();
            invalid.as_object_mut().unwrap().remove(key);
            assert!(SystemEvent::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut typed = event();
        typed.timestamp = "2026-10-01T00:00:00+00:00".into();
        typed.operation_id = Some(uuid::Uuid::new_v4().to_string());
        typed.account_id = Some(uuid::Uuid::new_v4().to_string());
        for status in [100, 599] {
            typed.http_status = Some(status);
            typed.validate().unwrap();
        }
        let mut oversized = serde_json::to_vec(&value).unwrap();
        oversized.extend(vec![b' '; MAX_EVENT_BYTES]);
        assert!(matches!(
            SystemEvent::from_json(&oversized),
            Err(SYSTEM_LOG_INVALID)
        ));
    }
    #[test]
    fn diagnostics_preserve_existing_strict_shape() {
        let mut value = serde_json::to_value(event()).unwrap();
        value["diagnostics"] = crate::diagnostics::fixture();
        assert!(SystemEvent::from_json(&serde_json::to_vec(&value).unwrap()).is_ok());
        value["diagnostics"]["password"] = serde_json::json!("FAKE");
        assert!(matches!(
            SystemEvent::from_json(&serde_json::to_vec(&value).unwrap()),
            Err(SYSTEM_LOG_INVALID)
        ));
    }
    #[test]
    fn retention_cutoff_future_clock_rollback_and_bounds() {
        let now = DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut store = Store::default();
        for time in [
            "2026-09-23T23:59:59Z",
            "2026-09-24T00:00:00Z",
            "2026-09-30T00:00:00Z",
            "2027-01-01T00:00:00Z",
        ] {
            let mut e = event();
            e.timestamp = time.into();
            store.items.push(e);
        }
        store.prune(now, 7);
        assert_eq!(store.items.len(), 3);
        assert_eq!(store.dropped_count, 1);
        store.prune(now - chrono::Duration::days(30), 7);
        assert_eq!(store.items.len(), 3);
        let mut large = Store {
            items: vec![event(); MAX_ITEMS + 1],
            ..Store::default()
        };
        large.bound().unwrap();
        assert_eq!(large.items.len(), MAX_ITEMS);
        assert_eq!(large.dropped_count, 1);
        // Current finite metadata may hit the row cap before 8MiB. Exercise the
        // same production byte-trimming algorithm at a smaller test-only cap.
        let mut e = event();
        e.diagnostics = Some(serde_json::from_value(crate::diagnostics::fixture()).unwrap());
        e.operation_id = Some(uuid::Uuid::new_v4().to_string());
        e.account_id = e.operation_id.clone();
        e.elapsed_ms = Some(u64::MAX);
        e.http_status = Some(599);
        e.stage = Some(EventStage::BrowserSpawn);
        e.reason = Some(EventReason::UnsupportedResponse);
        let mut bytes = Store {
            items: vec![e; 100],
            ..Store::default()
        };
        assert!(serde_json::to_vec(&bytes).unwrap().len() > 8192);
        bytes.bound_limits(MAX_ITEMS, 8192).unwrap();
        assert!(serde_json::to_vec(&bytes).unwrap().len() <= 8192);
        assert!(bytes.dropped_count > 0);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn queue_full_dropped_flush_reload_shutdown_and_page() {
        let (_tmp, path, settings, sink) = fixture().await;
        for _ in 0..QUEUE_CAPACITY {
            assert!(sink.emit(event()));
        }
        assert!(!sink.emit(event()));
        assert!(sink.shared.pending.lock().unwrap().bytes <= MAX_PENDING_BYTES);
        sink.flush().await.unwrap();
        let page = sink
            .page(SystemLogQuery {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.items.len(), QUEUE_CAPACITY);
        assert_eq!(page.dropped_count, 1);
        let value = serde_json::to_value(page).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        sink.shutdown().await.unwrap();
        assert!(!sink.emit(event()));
        let reopened = SystemLogSink::open(path, settings).await.unwrap();
        assert_eq!(reopened.page(Default::default()).unwrap().items.len(), 100);
        reopened.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn hot_filter_queries_and_settings_prune() {
        let (_tmp, _path, settings, sink) = fixture().await;
        for threshold in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            settings
                .update(LogSettings {
                    level: threshold,
                    ..Default::default()
                })
                .await
                .unwrap();
            for level in [
                LogLevel::Error,
                LogLevel::Warn,
                LogLevel::Info,
                LogLevel::Debug,
                LogLevel::Trace,
            ] {
                assert_eq!(
                    sink.emit(SystemEvent::new(level, EventKind::TaskStart)),
                    threshold.allows(level)
                );
            }
        }
        let mut tagged = event();
        tagged.account_id = Some(uuid::Uuid::new_v4().to_string());
        tagged.operation_id = Some(uuid::Uuid::new_v4().to_string());
        assert!(sink.emit(tagged.clone()));
        sink.flush().await.unwrap();
        let page = sink
            .page(SystemLogQuery {
                account_id: tagged.account_id,
                operation_id: tagged.operation_id,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let severe = sink
            .page(SystemLogQuery {
                level: Some(LogLevel::Warn),
                ..Default::default()
            })
            .unwrap();
        assert!(severe.items.iter().all(|e| e.level <= LogLevel::Warn));
        for limit in [0, 1001] {
            assert!(
                sink.page(SystemLogQuery {
                    limit: Some(limit),
                    ..Default::default()
                })
                .is_err()
            );
        }
        let mut old = event();
        old.timestamp = (Utc::now() - chrono::Duration::days(3)).to_rfc3339();
        assert!(sink.emit(old));
        sink.flush().await.unwrap();
        settings
            .update(LogSettings {
                system_retention_days: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        sink.flush().await.unwrap();
        assert!(
            sink.page(Default::default())
                .unwrap()
                .items
                .iter()
                .all(|e| DateTime::parse_from_rfc3339(&e.timestamp).unwrap()
                    > Utc::now() - chrono::Duration::days(1))
        );
        sink.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn clear_old_generation_pending_and_failed_clear_transaction() {
        let (_tmp, path, _settings, sink) = fixture().await;
        let old = sink.capture_generation();
        assert!(sink.emit(event()));
        sink.flush().await.unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(sink.clear().await.unwrap_err(), SYSTEM_LOG_ERROR);
        assert_eq!(sink.capture_generation().0, old.0);
        assert_eq!(sink.page(Default::default()).unwrap().items.len(), 1);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        sink.clear().await.unwrap();
        assert_eq!(sink.capture_generation().0, old.0 + 1);
        assert!(load(&path).unwrap().items.is_empty());
        assert!(sink.try_enqueue_captured(event(), old));
        assert!(sink.emit(event()));
        sink.flush().await.unwrap();
        let page = sink.page(Default::default()).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.dropped_count, 1);
        sink.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn startup_retention_private_modes_and_strict_load() {
        let (_tmp, path, settings, sink) = fixture().await;
        sink.shutdown().await.unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        let mut old = event();
        old.timestamp = (Utc::now() - chrono::Duration::days(8)).to_rfc3339();
        persist(
            &path,
            &Store {
                items: vec![old],
                ..Default::default()
            },
        )
        .unwrap();
        let reopened = SystemLogSink::open(path.clone(), settings.clone())
            .await
            .unwrap();
        assert!(reopened.page(Default::default()).unwrap().items.is_empty());
        reopened.shutdown().await.unwrap();
        for body in [
            br#"{"version":2,"items":[],"dropped_count":0}"#.as_slice(),
            br#"{"version":1,"items":[],"dropped_count":0,"raw":"FAKE"}"#,
        ] {
            fs::write(&path, body).unwrap();
            assert!(matches!(load(&path), Err(SYSTEM_LOG_INVALID)));
        }
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(path.parent().unwrap().join("missing"), &path).unwrap();
        assert!(matches!(
            SystemLogSink::open(path, settings).await,
            Err(SYSTEM_LOG_ERROR)
        ));
    }
    #[tokio::test(flavor = "current_thread")]
    async fn tracing_typed_only_target_and_trace_fake_secret_rejected() {
        use tracing_subscriber::prelude::*;
        let (_tmp, _path, settings, sink) = fixture().await;
        settings
            .update(LogSettings {
                level: LogLevel::Trace,
                ..Default::default()
            })
            .await
            .unwrap();
        let subscriber = tracing_subscriber::registry().with(SystemLogLayer::new(sink.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::trace!(target: "dependency", password = "FAKE-SECRET", "raw error");
            let typed = SystemEvent::new(LogLevel::Trace, EventKind::BrowserWait);
            emit_tracing(&typed).unwrap();
            let mut value = serde_json::to_value(typed).unwrap();
            value["password"] = serde_json::json!("FAKE-SECRET");
            let json = value.to_string();
            tracing::trace!(target: "anyrouter.system", event_json = json.as_str());
            tracing::trace!(target: "anyrouter.system", event_json = ?json);
            tracing::trace!(target: "anyrouter.system", event_json = json.as_str(), key = "FAKE-SECRET");
        });
        sink.flush().await.unwrap();
        let page = sink.page(Default::default()).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.dropped_count, 3);
        assert!(!serde_json::to_string(&page).unwrap().contains("FAKE"));
        sink.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn io_failure_nonblocking_enqueue_and_bounded_shutdown() {
        let (_tmp, path, _settings, sink) = fixture().await;
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(sink.emit(event()));
        assert_eq!(sink.flush().await.unwrap_err(), SYSTEM_LOG_ERROR);
        assert!(sink.failed());
        assert!(sink.emit(event()));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), sink.shutdown())
                .await
                .unwrap()
                .unwrap_err(),
            SYSTEM_LOG_ERROR
        );
        assert!(!sink.emit(event()));
    }
    #[tokio::test(start_paused = true)]
    async fn periodic_prune_without_new_events() {
        let (_tmp, path, _settings, sink) = fixture().await;
        let mut old = event();
        old.timestamp = (Utc::now() - chrono::Duration::days(8)).to_rfc3339();
        // Emulate the clock jumping forward while no admissions/settings change.
        sink.shared.store.lock().unwrap().items.push(old);
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::task::yield_now().await;
        let (reply, receive) = oneshot::channel();
        sink.sender
            .send(Message::Barrier(reply, Control::Flush))
            .await
            .unwrap();
        receive.await.unwrap().unwrap();
        assert!(load(&path).unwrap().items.is_empty());
        sink.shutdown().await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn byte_budget_and_control_capacity_timeouts_are_bounded() {
        let (_tmp, _path, _settings, original) = fixture().await;
        original.shutdown().await.unwrap();
        let (sender, _held_receiver) = mpsc::channel(1);
        let sink = SystemLogSink {
            sender,
            shared: Arc::new(Shared {
                store: Mutex::new(Store::default()),
                dropped: AtomicU64::new(0),
                failed: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                pending: Mutex::new(Pending {
                    closed: false,
                    bytes: MAX_PENDING_BYTES,
                }),
            }),
            settings: original.settings,
        };
        assert!(!sink.emit(event()));
        assert_eq!(sink.page(Default::default()).unwrap().dropped_count, 1);
        sink.shared.pending.lock().unwrap().bytes = 0;
        assert!(sink.emit(event()));
        assert_eq!(sink.flush().await.unwrap_err(), SYSTEM_LOG_ERROR);
        assert_eq!(sink.shutdown().await.unwrap_err(), SYSTEM_LOG_ERROR);
        // Timed-out reserve never publishes a closed flag without a barrier.
        assert!(!sink.shared.pending.lock().unwrap().closed);
    }
}
