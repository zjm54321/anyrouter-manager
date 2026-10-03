//! Private request metadata/error-prefix log. Never logs requests or secrets to stdout.
//! Error responses deliberately retain original text; invalid UTF-8 is replaced
//! with U+FFFD. Both raw prefix and resulting UTF-8 bytes are capped at 64 KiB.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot};

pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_ITEMS: usize = 1000;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const QUEUE_CAPACITY: usize = 256;
pub const MAX_PENDING_BYTES: usize = 1024 * 1024;
pub const LOG_ERROR: &str = "request_log_failed";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogEntry {
    pub id: String,
    pub timestamp: String,
    #[serde(deserialize_with = "required_option")]
    pub account_id: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub account_name: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub http_status: Option<u16>,
    #[serde(deserialize_with = "required_option")]
    pub error_body: Option<String>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarding_mode: Option<crate::responses_compat::ForwardingMode>,
}
fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
impl LogEntry {
    /// Construct at authenticated gateway admission, using its immutable route snapshot.
    pub fn new(account_id: Option<String>, account_name: Option<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
            account_id,
            account_name,
            http_status: None,
            error_body: None,
            truncated: false,
            forwarding_mode: None,
        }
    }
    pub fn set_response(&mut self, status: Option<u16>, capture: ErrorBodyCapture) {
        self.http_status = status;
        (self.error_body, self.truncated) =
            if status.is_some_and(|s| (100..600).contains(&s) && !(200..300).contains(&s)) {
                capture.finish()
            } else {
                (None, false)
            };
    }
    fn valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.id).is_ok()
            && self.id.len() == 36
            && chrono::DateTime::parse_from_rfc3339(&self.timestamp).is_ok()
            && self.timestamp.ends_with('Z')
            && self.timestamp.len() <= 40
            && self.account_id.as_ref().is_none_or(|s| s.len() <= 1024)
            && self.account_name.as_ref().is_none_or(|s| s.len() <= 4096)
            && self.http_status.is_none_or(|s| (100..600).contains(&s))
            && self
                .error_body
                .as_ref()
                .is_none_or(|s| s.len() <= MAX_BODY_BYTES)
            && (!self.truncated || self.error_body.is_some())
            && (self.http_status.is_some_and(|s| !(200..300).contains(&s))
                || (self.error_body.is_none() && !self.truncated))
    }
}

pub struct ErrorBodyCapture {
    enabled: bool,
    bytes: Vec<u8>,
    truncated: bool,
}
impl ErrorBodyCapture {
    pub fn new(status: Option<u16>) -> Self {
        Self {
            enabled: status.is_some_and(|s| (100..600).contains(&s) && !(200..300).contains(&s)),
            bytes: Vec::new(),
            truncated: false,
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn push(&mut self, chunk: &[u8]) {
        if !self.enabled {
            return;
        }
        let take = chunk.len().min(MAX_BODY_BYTES - self.bytes.len());
        self.bytes.extend_from_slice(&chunk[..take]);
        self.truncated |= take < chunk.len();
    }
    /// Call on stream error or client drop before EOF. Successful responses
    /// remain disabled and retain neither body nor a truncation flag.
    pub fn mark_incomplete(&mut self) {
        if self.enabled {
            self.truncated = true;
        }
    }
    /// Finish at EOF, or after `mark_incomplete` on premature termination.
    /// No stream is owned here; callers must distinguish EOF from error/drop.
    pub fn finish(self) -> (Option<String>, bool) {
        if !self.enabled {
            return (None, false);
        }
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        let mut end = text.len().min(MAX_BODY_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = self.truncated || end < text.len();
        text.truncate(end);
        (Some(text), truncated)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u8,
    items: Vec<LogEntry>,
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
    fn prune(&mut self, days: u8, now: chrono::DateTime<chrono::Utc>) {
        let cutoff = now - chrono::Duration::days(i64::from(days));
        let before = self.items.len();
        self.items.retain(|entry| {
            chrono::DateTime::parse_from_rfc3339(&entry.timestamp).is_ok_and(|time| time >= cutoff)
        });
        self.add_dropped((before - self.items.len()) as u64);
    }
    fn add_dropped(&mut self, count: u64) {
        self.dropped_count = self.dropped_count.saturating_add(count);
    }
    fn append(&mut self, entry: LogEntry) -> Result<(), &'static str> {
        self.items.push(entry);
        // Responses can finish out of order; retain/admit/page by request time.
        self.sort();
        self.enforce_bounds()
    }
    fn sort(&mut self) {
        self.items.sort_by(|a, b| {
            chrono::DateTime::parse_from_rfc3339(&a.timestamp)
                .ok()
                .cmp(&chrono::DateTime::parse_from_rfc3339(&b.timestamp).ok())
                .then_with(|| a.id.cmp(&b.id))
        });
    }
    fn enforce_bounds(&mut self) -> Result<(), &'static str> {
        while self.items.len() > MAX_ITEMS
            || serde_json::to_vec(self).map_err(|_| LOG_ERROR)?.len() > MAX_FILE_BYTES
        {
            self.items.remove(0);
            self.add_dropped(1);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct LogPage {
    pub items: Vec<LogEntry>,
    pub dropped_count: u64,
}

fn parent(path: &Path) -> Result<&Path, &'static str> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(LOG_ERROR)
}
fn private_dir(path: &Path) -> Result<(), &'static str> {
    let meta = fs::symlink_metadata(path).map_err(|_| LOG_ERROR)?;
    if !meta.is_dir() || meta.permissions().mode() & 0o777 != 0o700 {
        return Err(LOG_ERROR);
    }
    Ok(())
}
fn load(path: &Path) -> Result<Store, &'static str> {
    let directory = parent(path)?;
    // Missing logs are empty, but an existing parent must still be private.
    match fs::symlink_metadata(directory) {
        Ok(_) => private_dir(directory)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(LOG_ERROR),
    }
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(_) => return Err(LOG_ERROR),
    };
    private_dir(directory)?;
    let meta = file.metadata().map_err(|_| LOG_ERROR)?;
    if !meta.is_file()
        || meta.permissions().mode() & 0o777 != 0o600
        || meta.len() > MAX_FILE_BYTES as u64
    {
        return Err(LOG_ERROR);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| LOG_ERROR)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(LOG_ERROR);
    }
    let mut store: Store = serde_json::from_slice(&bytes).map_err(|_| LOG_ERROR)?;
    if store.version != 1
        || store.items.len() > MAX_ITEMS
        || !store.items.iter().all(LogEntry::valid)
    {
        return Err(LOG_ERROR);
    }
    store.sort();
    Ok(store)
}
fn persist(path: &Path, store: &Store) -> Result<(), &'static str> {
    persist_before_rename(path, store, || Ok(()))
}
fn persist_before_rename(
    path: &Path,
    store: &Store,
    before_rename: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), &'static str> {
    let directory = parent(path)?;
    if !directory.exists() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|_| LOG_ERROR)?;
    }
    private_dir(directory)?;
    if let Ok(meta) = fs::symlink_metadata(path)
        && (!meta.is_file() || meta.permissions().mode() & 0o777 != 0o600)
    {
        return Err(LOG_ERROR);
    }
    let bytes = serde_json::to_vec(store).map_err(|_| LOG_ERROR)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(LOG_ERROR);
    }
    let tmp = directory.join(format!(".request-log-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let dir = File::open(directory)?;
        dir.sync_all()?;
        before_rename()?;
        fs::rename(&tmp, path)?;
        let _ = dir.sync_all();
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result.map_err(|_| LOG_ERROR)
}

enum Message {
    Append(LogEntry, usize, u64),
    Barrier(oneshot::Sender<Result<(), &'static str>>, bool),
    Clear(oneshot::Sender<Result<(), &'static str>>),
    Prune,
}
struct Pending {
    closed: bool,
    bytes: usize,
}
struct Shared {
    store: Mutex<Store>,
    dropped: AtomicU64,
    failed: AtomicBool,
    pending: Mutex<Pending>,
    generation: AtomicU64,
    retention_days: AtomicU8,
    wake: tokio::sync::Notify,
    #[cfg(test)]
    fail_clear_rename: AtomicBool,
}
#[derive(Clone)]
pub struct LogSink {
    sender: mpsc::Sender<Message>,
    shared: Arc<Shared>,
}
impl LogSink {
    #[cfg(test)]
    pub async fn open(path: PathBuf) -> Result<Self, &'static str> {
        Self::open_with_retention_days(path, 7).await
    }
    /// Pass persisted settings at startup, before pruning; increasing retention
    /// after default `open` cannot restore already-expired records.
    #[allow(dead_code)] // Startup settings integration belongs to the caller.
    pub async fn open_with_retention_days(path: PathBuf, days: u8) -> Result<Self, &'static str> {
        Self::open_internal(
            path,
            days,
            chrono::Utc::now,
            std::time::Duration::from_secs(60),
        )
        .await
    }
    async fn open_internal(
        path: PathBuf,
        days: u8,
        clock: fn() -> chrono::DateTime<chrono::Utc>,
        prune_interval: std::time::Duration,
    ) -> Result<Self, &'static str> {
        if !(1..=90).contains(&days) {
            return Err(LOG_ERROR);
        }
        let source = path.clone();
        let store = tokio::task::spawn_blocking(move || {
            let mut store = load(&source)?;
            let before = store.items.len();
            store.prune(days, clock());
            if before != store.items.len() {
                persist(&source, &store)?;
            }
            Ok::<_, &'static str>(store)
        })
        .await
        .map_err(|_| LOG_ERROR)??;
        let shared = Arc::new(Shared {
            store: Mutex::new(store),
            dropped: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            retention_days: AtomicU8::new(days),
            wake: tokio::sync::Notify::new(),
            #[cfg(test)]
            fail_clear_rename: AtomicBool::new(false),
            pending: Mutex::new(Pending {
                closed: false,
                bytes: 0,
            }),
        });
        let (sender, mut receiver) = mpsc::channel(QUEUE_CAPACITY);
        let worker = shared.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(prune_interval);
            timer.tick().await;
            loop {
                let message = tokio::select! {
                    message = receiver.recv() => match message { Some(message) => message, None => break },
                    _ = timer.tick() => Message::Prune,
                    _ = worker.wake.notified() => Message::Prune,
                };
                let (entry, size, barrier, clear) = match message {
                    Message::Append(entry, size, generation) => {
                        (Some((entry, generation)), size, None, false)
                    }
                    Message::Barrier(reply, shutdown) => (None, 0, Some((reply, shutdown)), false),
                    Message::Clear(reply) => (None, 0, Some((reply, false)), true),
                    Message::Prune => (None, 0, None, false),
                };
                let state = worker.clone();
                let target = path.clone();
                let result = tokio::task::spawn_blocking(move || {
                    if clear {
                        let generation = state
                            .generation
                            .load(Ordering::Acquire)
                            .checked_add(1)
                            .ok_or(LOG_ERROR)?;
                        let empty = Store::default();
                        persist_before_rename(&target, &empty, || {
                            #[cfg(test)]
                            if state.fail_clear_rename.load(Ordering::Acquire) {
                                // Exercise the actual OS rename failure, not a post-commit error.
                                fs::rename(&target, target.join("invalid-child"))?;
                            }
                            Ok(())
                        })?;
                        *state.store.lock().map_err(|_| LOG_ERROR)? = empty;
                        state.dropped.store(0, Ordering::Release);
                        state.generation.store(generation, Ordering::Release);
                        return Ok(());
                    }
                    let mut snapshot = state.store.lock().map_err(|_| LOG_ERROR)?.clone();
                    let dropped = state.dropped.swap(0, Ordering::AcqRel);
                    snapshot.add_dropped(dropped);
                    if let Some((entry, generation)) = entry {
                        if generation == state.generation.load(Ordering::Acquire) {
                            snapshot.append(entry)?;
                        } else {
                            snapshot.add_dropped(1);
                        }
                    }
                    snapshot.prune(state.retention_days.load(Ordering::Acquire), clock());
                    snapshot.enforce_bounds()?;
                    if let Err(error) = persist(&target, &snapshot) {
                        let _ =
                            state
                                .dropped
                                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                                    Some(n.saturating_add(dropped))
                                });
                        return Err(error);
                    }
                    *state.store.lock().map_err(|_| LOG_ERROR)? = snapshot;
                    Ok(())
                })
                .await
                .unwrap_or(Err(LOG_ERROR));
                if let Ok(mut pending) = worker.pending.lock() {
                    pending.bytes -= size;
                } else {
                    worker.failed.store(true, Ordering::Release);
                }
                if result.is_err() {
                    worker.failed.store(true, Ordering::Release);
                }
                if let Some((reply, shutdown)) = barrier {
                    let _ = reply.send(if !clear && worker.failed.load(Ordering::Acquire) {
                        Err(LOG_ERROR)
                    } else {
                        result
                    });
                    if shutdown {
                        break;
                    }
                }
            }
        });
        Ok(Self { sender, shared })
    }
    /// Immediate queue admission only; never waits for filesystem/network/stream.
    /// False means dropped (invalid entry, full queue or closed sink).
    #[allow(dead_code)] // Compatibility API; gateway uses admission generation explicitly.
    pub fn try_enqueue(&self, entry: LogEntry) -> bool {
        self.try_enqueue_for(self.generation(), entry)
    }
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
    pub fn try_enqueue_for(&self, generation: u64, entry: LogEntry) -> bool {
        let size = if entry.valid() {
            serde_json::to_vec(&entry).map_or(MAX_PENDING_BYTES + 1, |bytes| bytes.len())
        } else {
            MAX_PENDING_BYTES + 1
        };
        let Ok(mut pending) = self.shared.pending.lock() else {
            self.shared.failed.store(true, Ordering::Release);
            return false;
        };
        if pending.closed
            || generation != self.generation()
            || size > MAX_PENDING_BYTES - pending.bytes
            || self
                .sender
                .try_send(Message::Append(entry, size, generation))
                .is_err()
        {
            let _ = self
                .shared
                .dropped
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    Some(n.saturating_add(1))
                });
            return false;
        }
        pending.bytes += size;
        true
    }
    pub fn page(&self, limit: Option<usize>) -> Result<LogPage, &'static str> {
        let store = self.shared.store.lock().map_err(|_| LOG_ERROR)?;
        Ok(LogPage {
            items: store
                .items
                .iter()
                .rev()
                .take(limit.unwrap_or(100).min(MAX_ITEMS))
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
    #[allow(dead_code)] // Management integration is owned by the caller's next phase.
    pub fn set_retention_days(&self, days: u8) -> Result<(), &'static str> {
        if !(1..=90).contains(&days) {
            return Err(LOG_ERROR);
        }
        self.shared.retention_days.store(days, Ordering::Release);
        self.shared.wake.notify_one();
        Ok(())
    }
    #[allow(dead_code)] // Management integration is owned by the caller's next phase.
    pub async fn clear(&self) -> Result<(), &'static str> {
        let permit = self.sender.reserve().await.map_err(|_| LOG_ERROR)?;
        let (reply, receive) = oneshot::channel();
        {
            let pending = self.shared.pending.lock().map_err(|_| LOG_ERROR)?;
            if pending.closed {
                return Err(LOG_ERROR);
            }
            permit.send(Message::Clear(reply));
        }
        receive.await.map_err(|_| LOG_ERROR)?
    }
    pub async fn flush(&self) -> Result<(), &'static str> {
        self.barrier(false).await
    }
    pub async fn shutdown(&self) -> Result<(), &'static str> {
        self.barrier(true).await
    }
    async fn barrier(&self, shutdown: bool) -> Result<(), &'static str> {
        // Reserve before closing: cancellation while waiting for capacity cannot
        // strand a closed sink without its shutdown message in the queue.
        let permit = self.sender.reserve().await.map_err(|_| LOG_ERROR)?;
        let (reply, receive) = oneshot::channel();
        {
            let mut pending = self.shared.pending.lock().map_err(|_| LOG_ERROR)?;
            if pending.closed {
                return Err(LOG_ERROR);
            }
            if shutdown {
                pending.closed = true;
            }
            permit.send(Message::Barrier(reply, shutdown));
        }
        receive.await.map_err(|_| LOG_ERROR)?
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn forwarding_mode_is_optional_for_old_logs_and_strict_when_present() {
        let entry = LogEntry::new(None, None);
        let mut value = serde_json::to_value(&entry).unwrap();
        assert!(value.get("forwarding_mode").is_none());
        assert_eq!(
            serde_json::from_value::<LogEntry>(value.clone()).unwrap(),
            entry
        );
        value["forwarding_mode"] = serde_json::json!("adapt");
        assert_eq!(
            serde_json::from_value::<LogEntry>(value.clone())
                .unwrap()
                .forwarding_mode,
            Some(crate::responses_compat::ForwardingMode::Adapt)
        );
        value["forwarding_mode"] = serde_json::json!("auto");
        assert!(serde_json::from_value::<LogEntry>(value).is_err());
    }
    use super::*;
    fn fixed() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }
    fn aged(days: i64) -> LogEntry {
        let mut entry = LogEntry::new(None, None);
        entry.timestamp = (fixed() - chrono::Duration::days(days))
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        entry
    }
    #[tokio::test]
    async fn retention_startup_settings_bounds_future_and_known_state_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log.json");
        let mut store = Store::default();
        for days in [91, 8, 7, 1, -3] {
            store.append(aged(days)).unwrap();
        }
        persist(&path, &store).unwrap();
        let sink =
            LogSink::open_internal(path.clone(), 7, fixed, std::time::Duration::from_secs(60))
                .await
                .unwrap();
        assert_eq!(sink.page(None).unwrap().items.len(), 3);
        assert_eq!(load(&path).unwrap().dropped_count, 2);
        assert_eq!(sink.set_retention_days(0), Err(LOG_ERROR));
        assert_eq!(sink.set_retention_days(91), Err(LOG_ERROR));
        sink.set_retention_days(1).unwrap();
        sink.flush().await.unwrap();
        let page = sink.page(None).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.dropped_count, 3);
        assert!(page.items.iter().any(|e| e.timestamp == aged(-3).timestamp));
        sink.set_retention_days(90).unwrap();
        assert!(sink.try_enqueue(aged(89)));
        sink.flush().await.unwrap();
        assert_eq!(sink.page(None).unwrap().items.len(), 3);
        sink.shutdown().await.unwrap();
        let reopened =
            LogSink::open_internal(path.clone(), 90, fixed, std::time::Duration::from_secs(60))
                .await
                .unwrap();
        assert_eq!(reopened.page(None).unwrap().items.len(), 3);
        let before = reopened.page(None).unwrap().items;
        // A non-directory destination fails deterministically even for privileged test runners.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        reopened.set_retention_days(1).unwrap();
        assert!(reopened.flush().await.is_err());
        assert_eq!(reopened.page(None).unwrap().items, before);
        let _ = reopened.shutdown().await;
    }

    #[tokio::test]
    async fn minute_prune_worker_can_be_tested_with_short_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log.json");
        let sink =
            LogSink::open_internal(path.clone(), 7, fixed, std::time::Duration::from_millis(5))
                .await
                .unwrap();
        // Model passage of time by placing an expired entry in last-known memory.
        sink.shared.store.lock().unwrap().append(aged(8)).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if sink.page(None).unwrap().items.is_empty() && path.exists() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(load(&path).unwrap().dropped_count, 1);
        sink.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clear_barrier_queued_and_late_old_generation_never_refill() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log.json");
        let sink = LogSink::open(path.clone()).await.unwrap();
        let generation = sink.generation();
        for _ in 0..20 {
            assert!(sink.try_enqueue_for(generation, LogEntry::new(None, None)));
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), sink.clear())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sink.generation(), generation + 1);
        assert!(sink.page(None).unwrap().items.is_empty());
        assert_eq!(sink.page(None).unwrap().dropped_count, 0);
        assert!(!sink.try_enqueue_for(generation, error(b"old in-flight")));
        // Queued before commit, but processed after clear: writer checks again.
        sink.sender
            .send(Message::Append(error(b"old queued"), 0, generation))
            .await
            .unwrap();
        sink.flush().await.unwrap();
        assert!(sink.page(None).unwrap().items.is_empty());
        assert_eq!(sink.page(None).unwrap().dropped_count, 2);
        assert!(sink.try_enqueue(error(b"new generation")));
        sink.shutdown().await.unwrap();
        assert_eq!(
            load(&path).unwrap().items[0].error_body.as_deref(),
            Some("new generation")
        );
    }

    #[tokio::test]
    async fn clear_rename_failure_preserves_file_memory_and_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("private");
        let path = directory.join("log.json");
        let sink = LogSink::open(path.clone()).await.unwrap();
        assert!(sink.try_enqueue(error(b"original")));
        sink.flush().await.unwrap();
        let bytes = fs::read(&path).unwrap();
        let entries = sink.page(None).unwrap().items;
        let generation = sink.generation();
        sink.shared.fail_clear_rename.store(true, Ordering::Release);
        assert_eq!(sink.clear().await, Err(LOG_ERROR));
        assert_eq!(sink.generation(), generation);
        assert_eq!(sink.page(None).unwrap().items, entries);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let _ = sink.shutdown().await;
    }
    fn error(body: &[u8]) -> LogEntry {
        let mut entry = LogEntry::new(Some("fixture-id".into()), Some("fixture-name".into()));
        let mut capture = ErrorBodyCapture::new(Some(500));
        capture.push(body);
        entry.set_response(Some(500), capture);
        entry
    }
    #[test]
    fn successful_unknown_status_no_body_and_exact_dto() {
        for status in [None, Some(200), Some(201), Some(204), Some(299)] {
            let mut capture = ErrorBodyCapture::new(status);
            assert!(!capture.enabled());
            capture.push(b"FAKE-password-prompt-key");
            assert_eq!(capture.finish(), (None, false));
            let entry = LogEntry::new(None, None);
            let json = serde_json::to_value(entry).unwrap();
            assert_eq!(json.as_object().unwrap().len(), 7);
            assert!(json["http_status"].is_null());
            assert!(json["account_id"].is_null());
            assert!(!json.to_string().contains("FAKE"));
            let mut entry = LogEntry::new(Some("snapshot-id".into()), Some("snapshot-name".into()));
            let mut unexpected = ErrorBodyCapture::new(Some(500));
            unexpected.push(b"FAKE-success-body");
            entry.set_response(status, unexpected);
            assert_eq!(entry.http_status, status);
            assert_eq!(entry.error_body, None);
            assert!(!entry.truncated);
            assert_eq!(entry.account_id.as_deref(), Some("snapshot-id"));
            assert!(entry.timestamp.ends_with('Z') && uuid::Uuid::parse_str(&entry.id).is_ok());
        }
    }
    #[test]
    fn error_original_utf8_and_lossy_caps() {
        let mut redirect = ErrorBodyCapture::new(Some(302));
        assert!(redirect.enabled());
        redirect.push(b"redirect-FAKE-body");
        assert_eq!(redirect.finish().0.as_deref(), Some("redirect-FAKE-body"));
        assert_eq!(
            error(b"FAKE-secret?key=FAKE\nraw").error_body.unwrap(),
            "FAKE-secret?key=FAKE\nraw"
        );
        let mut capture = ErrorBodyCapture::new(Some(403));
        capture.push(&vec![0xff; MAX_BODY_BYTES]);
        capture.push(b"more");
        let (text, truncated) = capture.finish();
        let text = text.unwrap();
        assert!(text.len() <= MAX_BODY_BYTES && truncated && text.contains('\u{fffd}'));
        let mut capture = ErrorBodyCapture::new(Some(400));
        capture.push(&vec![b'a'; MAX_BODY_BYTES - 1]);
        capture.push("界".as_bytes());
        let (text, truncated) = capture.finish();
        assert!(text.unwrap().len() <= MAX_BODY_BYTES && truncated);
    }
    #[test]
    fn incomplete_error_preserves_short_or_empty_original_prefix() {
        for status in [302, 400, 500] {
            let mut complete = ErrorBodyCapture::new(Some(status));
            complete.push(b"FAKE-raw-prefix");
            assert_eq!(complete.finish(), (Some("FAKE-raw-prefix".into()), false));
            for prefix in [b"FAKE-raw-prefix".as_slice(), b""] {
                let mut capture = ErrorBodyCapture::new(Some(status));
                capture.push(prefix);
                capture.mark_incomplete();
                capture.mark_incomplete();
                assert_eq!(
                    capture.finish(),
                    (Some(String::from_utf8(prefix.to_vec()).unwrap()), true)
                );
            }
        }
        let mut capture = ErrorBodyCapture::new(Some(500));
        capture.push(&vec![b'x'; MAX_BODY_BYTES + 1]);
        capture.mark_incomplete();
        assert_eq!(capture.finish(), (Some("x".repeat(MAX_BODY_BYTES)), true));
    }
    #[test]
    fn incomplete_success_or_unknown_status_never_captures() {
        for status in std::iter::once(None).chain((200..300).map(Some)) {
            let mut capture = ErrorBodyCapture::new(status);
            capture.mark_incomplete();
            capture.push(b"FAKE-body-never-retained");
            capture.mark_incomplete();
            assert!(!capture.enabled());
            assert!(capture.bytes.is_empty());
            assert_eq!(capture.finish(), (None, false));
        }
    }
    #[test]
    fn bounded_count_and_serialized_size_preserve_newest() {
        let mut store = Store::default();
        for _ in 0..1001 {
            store.append(LogEntry::new(None, None)).unwrap();
        }
        assert_eq!(store.items.len(), 1000);
        assert_eq!(store.dropped_count, 1);
        for _ in 0..30 {
            store.append(error(&vec![b'"'; MAX_BODY_BYTES])).unwrap();
        }
        assert!(serde_json::to_vec(&store).unwrap().len() <= MAX_FILE_BYTES);
        assert_eq!(
            store
                .items
                .last()
                .unwrap()
                .error_body
                .as_ref()
                .unwrap()
                .len(),
            MAX_BODY_BYTES
        );
        assert!(store.dropped_count > 1);
    }

    #[test]
    fn completion_order_and_legacy_fraction_precision_do_not_reorder_requests() {
        let mut store = Store::default();
        let mut later = LogEntry::new(None, None);
        later.timestamp = "2026-10-01T01:00:00.123000001Z".into();
        let mut earlier = LogEntry::new(None, None);
        earlier.timestamp = "2026-10-01T01:00:00.123Z".into();
        store.append(later.clone()).unwrap();
        store.append(earlier.clone()).unwrap();
        assert_eq!(store.items, vec![earlier, later]);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log.json");
        persist(&path, &store).unwrap();
        assert_eq!(load(&path).unwrap().items, store.items);
    }
    #[test]
    fn strict_load_permissions_atomic_and_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log.json");
        assert_eq!(load(&path).unwrap().items.len(), 0);
        let mut store = Store::default();
        store.append(error(b"FAKE-private-body")).unwrap();
        persist(&path, &store).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(load(&path).unwrap().items, store.items);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
        assert_eq!(persist(&path, &store).unwrap_err(), LOG_ERROR);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        for body in [
            r#"{"version":2,"items":[],"dropped_count":0}"#,
            r#"{"version":1,"items":[],"dropped_count":0,"secret":"FAKE"}"#,
            "FAKE-invalid-json",
        ] {
            fs::write(&path, body).unwrap();
            assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
        }
        assert_eq!(load(Path::new("no-parent")).unwrap_err(), LOG_ERROR);
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("missing"), &path).unwrap();
        assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn queue_overflow_flush_reload_shutdown_and_page() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log.json");
        let sink = LogSink::open(path.clone()).await.unwrap();
        for _ in 0..QUEUE_CAPACITY {
            assert!(sink.try_enqueue(LogEntry::new(None, None)));
        }
        assert!(!sink.try_enqueue(LogEntry::new(None, None)));
        sink.flush().await.unwrap();
        assert_eq!(sink.page(None).unwrap().items.len(), 100);
        assert_eq!(sink.page(Some(2000)).unwrap().items.len(), QUEUE_CAPACITY);
        assert_eq!(sink.page(None).unwrap().dropped_count, 1);
        assert!(sink.try_enqueue(error(b"latest-FAKE")));
        sink.shutdown().await.unwrap();
        assert!(!sink.failed());
        assert!(!sink.try_enqueue(LogEntry::new(None, None)));
        let reopened = LogSink::open(path).await.unwrap();
        assert_eq!(
            reopened.page(Some(1)).unwrap().items[0]
                .error_body
                .as_deref(),
            Some("latest-FAKE")
        );
        assert_eq!(reopened.page(None).unwrap().dropped_count, 1);
        reopened.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn persist_failure_fixed_flag_no_body_reflection() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log.json");
        let sink = LogSink::open(path.clone()).await.unwrap();
        fs::create_dir_all(&path).unwrap();
        assert!(sink.try_enqueue(error(b"FAKE-secret-body")));
        assert_eq!(sink.flush().await.unwrap_err(), LOG_ERROR);
        assert!(sink.failed());
        assert_eq!(sink.shutdown().await.unwrap_err(), LOG_ERROR);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queue_byte_limit_and_invalid_entries_count_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = LogSink::open(tmp.path().join("private/log.json"))
            .await
            .unwrap();
        let entry = error(&vec![b'"'; MAX_BODY_BYTES]);
        let size = serde_json::to_vec(&entry).unwrap().len();
        let admitted = MAX_PENDING_BYTES / size;
        assert!(admitted < QUEUE_CAPACITY);
        for _ in 0..admitted {
            assert!(sink.try_enqueue(entry.clone()));
        }
        assert!(!sink.try_enqueue(entry));
        let mut invalid = LogEntry::new(None, None);
        invalid.timestamp = "not-a-date-FAKE".into();
        assert!(!sink.try_enqueue(invalid));
        assert!(sink.shared.pending.lock().unwrap().bytes <= MAX_PENDING_BYTES);
        sink.flush().await.unwrap();
        assert_eq!(sink.page(None).unwrap().dropped_count, 2);
        sink.shutdown().await.unwrap();
    }

    #[test]
    fn strict_entry_limits_schema_and_directory_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("private");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("log.json");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
        assert_eq!(persist(&path, &Store::default()).unwrap_err(), LOG_ERROR);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let mut store = Store::default();
        store.items.push(LogEntry::new(None, None));
        let mut json = serde_json::to_value(&store).unwrap();
        json["items"][0]
            .as_object_mut()
            .unwrap()
            .remove("account_id");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
        store.items[0].error_body = Some("FAKE-success-body".into());
        fs::write(&path, serde_json::to_vec(&store).unwrap()).unwrap();
        assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
        store.items = vec![LogEntry::new(None, None); MAX_ITEMS + 1];
        fs::write(&path, serde_json::to_vec(&store).unwrap()).unwrap();
        assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
        fs::write(&path, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
        assert_eq!(load(&path).unwrap_err(), LOG_ERROR);
        fs::remove_file(&path).unwrap();
        let bad_parent = tmp.path().join("symlink");
        std::os::unix::fs::symlink(&directory, &bad_parent).unwrap();
        assert_eq!(load(&bad_parent.join("log.json")).unwrap_err(), LOG_ERROR);
        assert_eq!(
            persist(&bad_parent.join("log.json"), &Store::default()).unwrap_err(),
            LOG_ERROR
        );
    }
}
