use crate::{
    app::{App, require_session},
    error::ApiError,
    log_settings::{LogLevel, LogSettings},
    system_log::{EventKind, SystemEvent, SystemLogPage, SystemLogQuery},
};
use axum::{
    Json,
    body::Bytes,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode},
};
use std::{sync::Arc, time::Duration};
fn bad() -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "bad_request")
}
fn unavailable() -> ApiError {
    crate::logging::report_failure(true);
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "logging_unavailable")
}
pub async fn get_settings(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<LogSettings>, ApiError> {
    require_session(&app, &headers).await?;
    Ok(Json(
        (*app
            .log_settings
            .as_ref()
            .ok_or_else(unavailable)?
            .snapshot())
        .clone(),
    ))
}
pub async fn put_settings(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<LogSettings>, ApiError> {
    require_session(&app, &headers).await?;
    let settings = LogSettings::from_json(&bytes).map_err(|_| bad())?;
    let saved = app
        .log_settings
        .as_ref()
        .ok_or_else(unavailable)?
        .update(settings)
        .await
        .map_err(|_| unavailable())?;
    if let Some(logs) = &app.logs {
        logs.set_retention_days(saved.request_retention_days as u8)
            .map_err(|_| unavailable())?;
    }
    if let Some(logs) = &app.system_logs {
        logs.emit(SystemEvent::new(LogLevel::Info, EventKind::SettingsUpdated));
    }
    Ok(Json((*saved).clone()))
}
fn query(raw: Option<String>) -> Result<SystemLogQuery, ApiError> {
    let mut result = SystemLogQuery::default();
    let mut seen = std::collections::HashSet::new();
    let raw = raw.unwrap_or_default();
    if raw.is_empty() {
        return Ok(result);
    }
    for field in raw.split('&') {
        let (key, value) = field.split_once('=').ok_or_else(bad)?;
        if !seen.insert(key) {
            return Err(bad());
        }
        match key {
            "limit" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                let n = value.parse().map_err(|_| bad())?;
                if !(1..=1000).contains(&n) {
                    return Err(bad());
                }
                result.limit = Some(n);
            }
            "level" => {
                result.level = Some(match value {
                    "error" => LogLevel::Error,
                    "warn" => LogLevel::Warn,
                    "info" => LogLevel::Info,
                    "debug" => LogLevel::Debug,
                    "trace" => LogLevel::Trace,
                    _ => return Err(bad()),
                })
            }
            "account_id" | "operation_id" => {
                if value.len() != 36 || uuid::Uuid::parse_str(value).is_err() {
                    return Err(bad());
                }
                if key == "account_id" {
                    result.account_id = Some(value.into());
                } else {
                    result.operation_id = Some(value.into());
                }
            }
            _ => return Err(bad()),
        }
    }
    Ok(result)
}
pub async fn get_system(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Json<SystemLogPage>, ApiError> {
    require_session(&app, &headers).await?;
    let query = query(raw)?;
    let sink = app.system_logs.as_ref().ok_or_else(unavailable)?;
    if sink.failed() {
        return Err(unavailable());
    }
    Ok(Json(sink.page(query).map_err(|_| unavailable())?))
}
pub async fn clear_system(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<StatusCode, ApiError> {
    require_session(&app, &headers).await?;
    if !bytes.is_empty() {
        return Err(bad());
    }
    let sink = app.system_logs.as_ref().ok_or_else(unavailable)?;
    tokio::time::timeout(Duration::from_secs(2), sink.clear())
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
    sink.emit(SystemEvent::new(LogLevel::Info, EventKind::LogCleared));
    Ok(StatusCode::NO_CONTENT)
}
pub async fn clear_request(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<StatusCode, ApiError> {
    require_session(&app, &headers).await?;
    if !bytes.is_empty() {
        return Err(bad());
    }
    tokio::time::timeout(
        Duration::from_secs(2),
        app.logs.as_ref().ok_or_else(unavailable)?.clear(),
    )
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())?;
    Ok(StatusCode::NO_CONTENT)
}
