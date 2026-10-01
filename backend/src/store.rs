use crate::{
    error::SafeError,
    model::{Active, id},
};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub fn load(path: &Path) -> Result<Option<Active>, &'static str> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot read saved state."),
    };
    if !meta.is_file() || meta.permissions().mode() & 0o777 != 0o600 || meta.len() > 1024 * 1024 {
        return Err("Saved state must be a regular 0600 file of supported size.");
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or("Saved state requires a private directory.")?;
    let directory =
        fs::symlink_metadata(parent).map_err(|_| "Cannot read saved state directory.")?;
    if !directory.is_dir() || directory.permissions().mode() & 0o777 != 0o700 {
        return Err("Saved state directory must be a regular 0700 directory.");
    }
    let active: Active =
        serde_json::from_slice(&fs::read(path).map_err(|_| "Cannot read saved state.")?)
            .map_err(|_| "Invalid saved state JSON.")?;
    if !active.validate() {
        return Err("Saved state contains invalid account material.");
    }
    Ok(Some(active))
}

pub fn save(path: &Path, active: &Active) -> Result<(), SafeError> {
    save_inner(path, active, |_| Ok(()))
}

fn save_inner(
    path: &Path,
    active: &Active,
    before_rename: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), SafeError> {
    let mut temp: Option<PathBuf> = None;
    let result = (|| -> std::io::Result<()> {
        if !active.validate() {
            return Err(std::io::Error::other("invalid state"));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| std::io::Error::other("state requires a private directory"))?;
        if !parent.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        if !fs::symlink_metadata(parent)?.is_dir() {
            return Err(std::io::Error::other("invalid state directory"));
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let tmp = parent.join(format!(".account-{}.tmp", id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        temp = Some(tmp.clone());
        let bytes = serde_json::to_vec(active)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        // All fallible preparation precedes rename. Rename is the commit point; callers
        // must swap their immutable snapshot immediately after this succeeds.
        let directory = fs::File::open(parent)?;
        directory.sync_all()?;
        before_rename(&tmp)?;
        fs::rename(&tmp, path)?;
        // A post-rename fsync failure cannot undo the commit. Keep memory consistent
        // with the renamed file rather than reporting a failed, unapplied activation.
        // This is an atomic visibility guarantee, not a promise of power-loss
        // durability when this best-effort post-rename directory sync fails.
        let _ = directory.sync_all();
        Ok(())
    })();
    if let Some(tmp) = temp {
        let _ = fs::remove_file(tmp);
    }
    result.map_err(|_| SafeError::new("persistence_failed"))
}

#[cfg(test)]
pub(crate) fn save_with_rename_failure(path: &Path, active: &Active) -> Result<(), SafeError> {
    save_inner(path, active, |tmp| {
        // Force the actual rename syscall to fail after a complete, synced write.
        fs::remove_file(tmp)
    })
}
