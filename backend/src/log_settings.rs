//! Independent, private log preferences. Never reads application credentials.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub const SETTINGS_ERROR: &str = "log_settings_failed";
pub const SETTINGS_INVALID: &str = "log_settings_invalid";
const MAX_SETTINGS_BYTES: usize = 1024;

/// Ordered most severe first. A threshold includes itself and more severe levels.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    pub fn allows(self, event: Self) -> bool {
        event <= self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "SettingsWire")]
pub struct LogSettings {
    pub level: LogLevel,
    pub system_retention_days: u16,
    pub request_retention_days: u16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsWire {
    level: LogLevel,
    system_retention_days: u16,
    request_retention_days: u16,
}
impl TryFrom<SettingsWire> for LogSettings {
    type Error = &'static str;
    fn try_from(value: SettingsWire) -> Result<Self, Self::Error> {
        let settings = Self {
            level: value.level,
            system_retention_days: value.system_retention_days,
            request_retention_days: value.request_retention_days,
        };
        settings.validate()?;
        Ok(settings)
    }
}
impl Default for LogSettings {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            system_retention_days: 7,
            request_retention_days: 7,
        }
    }
}
impl LogSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if (1..=90).contains(&self.system_retention_days)
            && (1..=90).contains(&self.request_retention_days)
        {
            Ok(())
        } else {
            Err(SETTINGS_INVALID)
        }
    }
    /// Route adapters must use this rather than reflecting serde's error text.
    pub fn from_json(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(SETTINGS_INVALID);
        }
        serde_json::from_slice(bytes).map_err(|_| SETTINGS_INVALID)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsStore {
    version: u8,
    settings: LogSettings,
}

struct Inner {
    path: PathBuf,
    current: watch::Sender<Arc<LogSettings>>,
    updates: Mutex<()>,
}
#[derive(Clone)]
pub struct SharedLogSettings(Arc<Inner>);
impl SharedLogSettings {
    /// `path` must be the separate log_settings.json, never application config.
    pub async fn open(path: PathBuf) -> Result<Self, &'static str> {
        let source = path.clone();
        let settings = tokio::task::spawn_blocking(move || {
            match read_private(&source, MAX_SETTINGS_BYTES).map_err(|_| SETTINGS_ERROR)? {
                Some(bytes) => {
                    let stored: SettingsStore =
                        serde_json::from_slice(&bytes).map_err(|_| SETTINGS_INVALID)?;
                    if stored.version != 1 {
                        return Err(SETTINGS_INVALID);
                    }
                    Ok(stored.settings)
                }
                None => {
                    let settings = LogSettings::default();
                    persist_settings(&source, &settings)?;
                    Ok(settings)
                }
            }
        })
        .await
        .map_err(|_| SETTINGS_ERROR)??;
        let (current, _) = watch::channel(Arc::new(settings));
        Ok(Self(Arc::new(Inner {
            path,
            current,
            updates: Mutex::new(()),
        })))
    }
    /// Immutable hot snapshot; no filesystem IO. Request-log adapters may read
    /// request_retention_days and subscribe without inheriting system severity.
    pub fn snapshot(&self) -> Arc<LogSettings> {
        self.0.current.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<Arc<LogSettings>> {
        self.0.current.subscribe()
    }
    /// Serializes persistence AND publication. Cancellation cannot publish an
    /// older snapshot after a newer persisted update: the blocking task owns the
    /// entire transaction, even if its caller stops awaiting it.
    pub async fn update(&self, settings: LogSettings) -> Result<Arc<LogSettings>, &'static str> {
        settings.validate()?;
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = inner.updates.lock().map_err(|_| SETTINGS_ERROR)?;
            persist_settings(&inner.path, &settings)?;
            let settings = Arc::new(settings);
            inner.current.send_replace(settings.clone());
            Ok(settings)
        })
        .await
        .map_err(|_| SETTINGS_ERROR)?
    }
}
fn persist_settings(path: &Path, settings: &LogSettings) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(&SettingsStore {
        version: 1,
        settings: settings.clone(),
    })
    .map_err(|_| SETTINGS_ERROR)?;
    atomic_private_write(path, &bytes).map_err(|_| SETTINGS_ERROR)
}

// These filesystem helpers expose only fixed errors to both log modules.
// Reject symlinks in all existing path components, not only the final file.
fn check_components(path: &Path) -> Result<(), ()> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(());
        }
        prefix.push(component.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(()),
        }
    }
    Ok(())
}
fn parent(path: &Path) -> Result<&Path, ()> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(())
}
fn private_dir(path: &Path) -> Result<(), ()> {
    let meta = fs::symlink_metadata(path).map_err(|_| ())?;
    if !meta.is_dir() || meta.permissions().mode() & 0o7777 != 0o700 {
        return Err(());
    }
    Ok(())
}
pub(crate) fn read_private(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, ()> {
    check_components(path)?;
    let directory = parent(path)?;
    match fs::symlink_metadata(directory) {
        Ok(_) => private_dir(directory)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    }
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let meta = file.metadata().map_err(|_| ())?;
    if !meta.is_file()
        || meta.permissions().mode() & 0o7777 != 0o600
        || meta.len() > max_bytes as u64
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > max_bytes {
        return Err(());
    }
    Ok(Some(bytes))
}
pub(crate) fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), ()> {
    check_components(path)?;
    let directory = parent(path)?;
    if !directory.exists() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|_| ())?;
    }
    private_dir(directory)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.permissions().mode() & 0o7777 != 0o600 => {
            return Err(());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(()),
    }
    let tmp = directory.join(format!(".log-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        let dir = File::open(directory)?;
        dir.sync_all()?;
        fs::rename(&tmp, path)?;
        // Rename is the commit point. A post-commit sync failure must not
        // incorrectly report a rollback while disk and hot state diverge.
        let _ = dir.sync_all();
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_defaults_thresholds_and_fixed_errors() {
        let defaults = LogSettings::default();
        assert_eq!(
            defaults,
            LogSettings::from_json(
                br#"{"level":"info","system_retention_days":7,"request_retention_days":7}"#
            )
            .unwrap()
        );
        let levels = [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ];
        for (i, threshold) in levels.iter().enumerate() {
            for (j, level) in levels.iter().enumerate() {
                assert_eq!(threshold.allows(*level), j <= i);
            }
        }
        for bytes in [
            br#"{"level":"secret","system_retention_days":7,"request_retention_days":7}"#.as_slice(),
            br#"{"level":"info","system_retention_days":0,"request_retention_days":7}"#,
            br#"{"level":"info","system_retention_days":91,"request_retention_days":7}"#,
            br#"{"level":"info","system_retention_days":7.0,"request_retention_days":7}"#,
            br#"{"level":"info","system_retention_days":7,"request_retention_days":7,"password":"FAKE"}"#,
            br#"{"level":"info","system_retention_days":7}"#,
        ] { assert_eq!(LogSettings::from_json(bytes).unwrap_err(), SETTINGS_INVALID); }
    }
    #[tokio::test]
    async fn update_commit_hot_watch_failure_keeps_old_and_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log_settings.json");
        let shared = SharedLogSettings::open(path.clone()).await.unwrap();
        let mut watch = shared.subscribe();
        let next = LogSettings {
            level: LogLevel::Trace,
            ..Default::default()
        };
        shared.update(next.clone()).await.unwrap();
        watch.changed().await.unwrap();
        assert_eq!(**watch.borrow(), next);
        assert_eq!(
            *SharedLogSettings::open(path.clone())
                .await
                .unwrap()
                .snapshot(),
            next
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            shared.update(LogSettings::default()).await.unwrap_err(),
            SETTINGS_ERROR
        );
        assert_eq!(*shared.snapshot(), next);
        assert!(!watch.has_changed().unwrap());
    }
    #[tokio::test]
    async fn concurrent_updates_disk_and_hot_never_disagree() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log_settings.json");
        let shared = SharedLogSettings::open(path.clone()).await.unwrap();
        let mut jobs = Vec::new();
        for days in 1..=20 {
            let shared = shared.clone();
            jobs.push(tokio::spawn(async move {
                shared
                    .update(LogSettings {
                        system_retention_days: days,
                        ..LogSettings::default()
                    })
                    .await
                    .unwrap();
            }));
        }
        for job in jobs {
            job.await.unwrap();
        }
        let reopened = SharedLogSettings::open(path).await.unwrap();
        assert_eq!(*shared.snapshot(), *reopened.snapshot());
    }
    #[tokio::test]
    async fn version_permissions_symlinks_and_atomic_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private/log_settings.json");
        SharedLogSettings::open(path.clone()).await.unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        let dir = path.parent().unwrap();
        assert_eq!(
            fs::metadata(dir).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(fs::read_dir(dir).unwrap().count(), 1);
        fs::write(&path, br#"{"version":2,"settings":{"level":"info","system_retention_days":7,"request_retention_days":7}}"#).unwrap();
        assert!(matches!(
            SharedLogSettings::open(path.clone()).await,
            Err(SETTINGS_INVALID)
        ));
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("absent"), &path).unwrap();
        assert!(matches!(
            SharedLogSettings::open(path.clone()).await,
            Err(SETTINGS_ERROR)
        ));
        fs::remove_file(&path).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            SharedLogSettings::open(path.clone()).await,
            Err(SETTINGS_ERROR)
        ));
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(dir, &link).unwrap();
        assert!(matches!(
            SharedLogSettings::open(link.join("log_settings.json")).await,
            Err(SETTINGS_ERROR)
        ));
    }
}
