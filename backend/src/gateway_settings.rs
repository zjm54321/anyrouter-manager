//! Private gateway preferences, independent of accounts and application config.
use crate::log_settings::{atomic_private_write, read_private};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub const MAX_SETTINGS_BYTES: usize = 1024;
pub const SETTINGS_ERROR: &str = "gateway_settings_failed";
pub const SETTINGS_INVALID: &str = "gateway_settings_invalid";

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GatewayMode {
    #[default]
    Pass,
    Adapt,
    Auto,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GatewaySettings {
    pub responses_mode: GatewayMode,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u8,
    settings: GatewaySettings,
}

struct Inner {
    path: PathBuf,
    current: watch::Sender<Arc<GatewaySettings>>,
    updates: Mutex<()>,
}
#[derive(Clone)]
pub struct SharedGatewaySettings(Arc<Inner>);

impl SharedGatewaySettings {
    #[cfg(test)]
    pub(crate) fn test_default(path: PathBuf) -> Self {
        let (current, _) = watch::channel(Arc::new(GatewaySettings::default()));
        Self(Arc::new(Inner {
            path,
            current,
            updates: Mutex::new(()),
        }))
    }
    pub async fn open(path: PathBuf) -> Result<Self, &'static str> {
        tokio::task::spawn_blocking(move || Self::open_private(path))
            .await
            .map_err(|_| SETTINGS_ERROR)?
    }

    // Production startup must validate/create the private store before admission.
    pub(crate) fn open_private(path: PathBuf) -> Result<Self, &'static str> {
        let settings = match read_private(&path, MAX_SETTINGS_BYTES).map_err(|_| SETTINGS_ERROR)? {
            Some(bytes) => {
                let stored: Stored =
                    serde_json::from_slice(&bytes).map_err(|_| SETTINGS_INVALID)?;
                if stored.version != 1 {
                    return Err(SETTINGS_INVALID);
                }
                stored.settings
            }
            None => {
                let settings = GatewaySettings::default();
                persist(&path, &settings)?;
                settings
            }
        };
        let (current, _) = watch::channel(Arc::new(settings));
        Ok(Self(Arc::new(Inner {
            path,
            current,
            updates: Mutex::new(()),
        })))
    }

    pub fn snapshot(&self) -> Arc<GatewaySettings> {
        self.0.current.borrow().clone()
    }

    pub async fn update(
        &self,
        settings: GatewaySettings,
    ) -> Result<Arc<GatewaySettings>, &'static str> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = inner.updates.lock().map_err(|_| SETTINGS_ERROR)?;
            persist(&inner.path, &settings)?;
            let settings = Arc::new(settings);
            inner.current.send_replace(settings.clone());
            Ok(settings)
        })
        .await
        .map_err(|_| SETTINGS_ERROR)?
    }
}

fn persist(path: &Path, settings: &GatewaySettings) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(&Stored {
        version: 1,
        settings: settings.clone(),
    })
    .map_err(|_| SETTINGS_ERROR)?;
    atomic_private_write(path, &bytes).map_err(|_| SETTINGS_ERROR)
}

pub async fn get(
    axum::extract::State(app): axum::extract::State<Arc<crate::app::App>>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<GatewaySettings>, crate::error::ApiError> {
    crate::app::require_session(&app, &headers).await?;
    Ok(axum::Json((*app.gateway_settings.snapshot()).clone()))
}

pub async fn put(
    axum::extract::State(app): axum::extract::State<Arc<crate::app::App>>,
    request: axum::extract::Request,
) -> Result<axum::Json<GatewaySettings>, crate::error::ApiError> {
    use crate::error::ApiError;
    use axum::http::StatusCode;
    crate::app::require_session(&app, request.headers()).await?;
    let bytes = tokio::time::timeout(
        crate::gateway::BODY_COLLECTION_TIMEOUT,
        axum::body::to_bytes(request.into_body(), MAX_SETTINGS_BYTES),
    )
    .await
    .map_err(|_| ApiError::new(StatusCode::REQUEST_TIMEOUT, "gateway_settings_timeout"))?
    .map_err(|_| ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "gateway_settings_too_large"))?;
    let settings = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, SETTINGS_INVALID))?;
    let saved = app
        .gateway_settings
        .update(settings)
        .await
        .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, SETTINGS_ERROR))?;
    Ok(axum::Json((*saved).clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[tokio::test]
    async fn private_defaults_reopen_and_failed_update_keep_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/gateway_settings.json");
        let store = SharedGatewaySettings::open(path.clone()).await.unwrap();
        let admitted = store.snapshot();
        assert_eq!(admitted.responses_mode, GatewayMode::Pass);
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
        store
            .update(GatewaySettings {
                responses_mode: GatewayMode::Adapt,
            })
            .await
            .unwrap();
        assert_eq!(admitted.responses_mode, GatewayMode::Pass);
        assert_eq!(
            SharedGatewaySettings::open(path.clone())
                .await
                .unwrap()
                .snapshot()
                .responses_mode,
            GatewayMode::Adapt
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            store.update(GatewaySettings::default()).await.unwrap_err(),
            SETTINGS_ERROR
        );
        assert_eq!(store.snapshot().responses_mode, GatewayMode::Adapt);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn corrupt_versions_unknown_fields_and_symlinks_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/gateway_settings.json");
        SharedGatewaySettings::open(path.clone()).await.unwrap();
        for bytes in [
            "broken",
            r#"{"version":2,"settings":{"responses_mode":"pass"}}"#,
            r#"{"version":1,"settings":{"responses_mode":"unknown"}}"#,
            r#"{"version":1,"settings":{"responses_mode":"pass","secret":"synthetic"}}"#,
            r#"{"version":1,"settings":{"responses_mode":"pass"},"extra":null}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(matches!(
                SharedGatewaySettings::open(path.clone()).await,
                Err(SETTINGS_INVALID)
            ));
            assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
        }
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("absent"), &path).unwrap();
        assert!(matches!(
            SharedGatewaySettings::open(path.clone()).await,
            Err(SETTINGS_ERROR)
        ));
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(path.parent().unwrap(), &alias).unwrap();
        assert!(matches!(
            SharedGatewaySettings::open(alias.join("other.json")).await,
            Err(SETTINGS_ERROR)
        ));
    }
}
