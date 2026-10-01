use crate::{
    error::SafeError,
    model::{Active, Portfolio, id},
};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub fn load(path: &Path) -> Result<Portfolio, &'static str> {
    load_inner(path, false)
}

fn load_inner(path: &Path, fail_migration: bool) -> Result<Portfolio, &'static str> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Portfolio::default()),
        Err(_) => return Err("Cannot read saved state."),
    };
    if !meta.is_file() || meta.permissions().mode() & 0o777 != 0o600 || meta.len() > 8 * 1024 * 1024
    {
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
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(path).map_err(|_| "Cannot read saved state.")?)
            .map_err(|_| "Invalid saved state JSON.")?;
    let legacy = value.get("version").is_none();
    let portfolio = if legacy {
        let active: Active = serde_json::from_value(value).map_err(|_| "Invalid legacy state.")?;
        if !active.validate() {
            return Err("Invalid legacy account material.");
        }
        Portfolio::from_active(active)
    } else {
        serde_json::from_value::<Portfolio>(value).map_err(|_| "Invalid portfolio state.")?
    };
    if !portfolio.validate() {
        return Err("Saved state contains invalid account material.");
    }
    if legacy {
        save_inner(path, &portfolio, |_| {
            if fail_migration {
                Err(std::io::Error::other("migration fault"))
            } else {
                Ok(())
            }
        })
        .map_err(|_| "Cannot migrate saved state.")?;
    }
    Ok(portfolio)
}

pub fn save(path: &Path, active: &Portfolio) -> Result<(), SafeError> {
    save_inner(path, active, |_| Ok(()))
}

fn save_inner(
    path: &Path,
    active: &Portfolio,
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
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(std::io::Error::other("state too large"));
        }
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
pub(crate) fn save_with_rename_failure(path: &Path, active: &Portfolio) -> Result<(), SafeError> {
    save_inner(path, active, |tmp| {
        // Force the actual rename syscall to fail after a complete, synced write.
        fs::remove_file(tmp)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_migration_preserves_material_and_failure_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("private");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let path = parent.join("account.json");
        let legacy = crate::tests::active();
        let bytes = serde_json::to_vec(&legacy).unwrap();
        fs::write(&path, &bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load_inner(&path, true).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let portfolio = load(&path).unwrap();
        assert_eq!(portfolio.version, 2);
        assert_eq!(portfolio.accounts.len(), 1);
        assert_eq!(
            serde_json::to_value(portfolio.route().unwrap().snapshot().unwrap()).unwrap(),
            serde_json::to_value(legacy).unwrap()
        );
        assert_eq!(portfolio.accounts[0].keys[0].id, "1");
        assert_eq!(
            serde_json::to_vec(&load(&path).unwrap()).unwrap(),
            serde_json::to_vec(&portfolio).unwrap()
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let mut invalid = portfolio.clone();
        invalid.accounts.push(invalid.accounts[0].clone());
        invalid.accounts[1].id = crate::model::id();
        assert!(!invalid.validate());
        let old = fs::read(&path).unwrap();
        assert!(save(&path, &invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), old);
        invalid = portfolio;
        invalid.version = 3;
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(load(&path).is_err());
    }
}
