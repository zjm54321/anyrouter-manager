//! Conservative single-attempt check-in. UTC midnight is the fixed UTC+8 08:00 boundary.
use crate::{
    app::{self, App},
    error::{ApiError, SafeError},
    model::{Active, Credentials},
    upstream::{UPSTREAM, identifier},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub time: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            time: "09:00".into(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Success,
    AlreadyDone,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    Scheduled,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum Code {
    #[serde(rename = "checkin_outcome_unknown")]
    OutcomeUnknown,
    #[serde(rename = "checkin_cycle_ambiguous")]
    CycleAmbiguous,
    #[serde(rename = "upstream_session_expired")]
    SessionExpired,
    #[serde(rename = "checkin_preflight_failed")]
    PreflightFailed,
    #[serde(rename = "persistence_failed")]
    PersistenceFailed,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub date: String,
    pub account_user_id: String,
    pub trigger: Trigger,
    pub status: Status,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub code: Option<Code>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub settings: Settings,
    pub history: Vec<Record>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, helper::LoginProvider, upstream::Upstream};
    use axum::{Router, routing::any};
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn fixed() -> DateTime<Utc> {
        at("2026-10-01T02:00:00Z")
    }
    struct Provider(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl LoginProvider for Provider {
        async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
            panic!("password login forbidden")
        }
        async fn verify_session(
            &self,
            c: Credentials,
            _: Duration,
        ) -> Result<Credentials, SafeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(c)
        }
    }
    #[test]
    fn cycle_schedule_boundaries_and_validation() {
        assert_eq!(cycle(at("2026-10-01T07:59:59+08:00")), "2026-09-30");
        assert_eq!(cycle(at("2026-10-01T08:00:00+08:00")), "2026-10-01");
        assert_eq!(cycle(at("2026-10-02T00:00:00+08:00")), "2026-10-01");
        assert_eq!(due(fixed(), "07:00"), at("2026-10-01T23:00:00Z"));
        assert_eq!(due(fixed(), "09:17"), at("2026-10-01T01:17:00Z"));
        for s in ["9:00", "24:00", "09:60", " 09:00", "é:00", "00:00:00"] {
            assert!(minutes(s).is_none());
        }
        assert!(!Saved::default().settings.enabled);
    }
    fn record() -> Record {
        Record {
            date: cycle(fixed()),
            account_user_id: "7".into(),
            trigger: Trigger::Manual,
            status: Status::Running,
            started_at: fixed().to_rfc3339(),
            finished_at: None,
            code: None,
        }
    }
    #[test]
    fn persistence_recovery_bounds_and_safe_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/checkin.json");
        assert!(load(&path).unwrap().history.is_empty());
        let mut s = Saved::default();
        s.put(record());
        save(&path, &s).unwrap();
        let recovered = load(&path).unwrap();
        assert!(recovered.history[0].status == Status::Unknown);
        assert!(load(&path).unwrap().history[0].status == Status::Unknown);
        for n in 0..200 {
            let mut r = record();
            r.date = cycle(fixed() + chrono::Duration::days(n));
            s.put(r);
        }
        assert_eq!(s.history.len(), 180);
        assert!(s.valid());
        let serialized = serde_json::to_string(&s).unwrap();
        for secret in ["cookies", "password", "username", "private-upstream-cookie"] {
            assert!(!serialized.contains(secret));
        }
        assert!(
            serde_json::from_str::<Saved>(&serialized.replace("running", "invented_status"))
                .is_err()
        );
        s.history[0].started_at = "invalid".into();
        assert!(!s.valid());
    }
    async fn fixture(
        mode: usize,
    ) -> (
        Arc<App>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        tempfile::TempDir,
        tokio::task::JoinHandle<()>,
    ) {
        let posts = Arc::new(AtomicUsize::new(0));
        let verifies = Arc::new(AtomicUsize::new(0));
        let gets = Arc::new(AtomicUsize::new(0));
        let count = posts.clone();
        let get_count = gets.clone();
        let router = Router::new().fallback(any(move |request: axum::extract::Request| {
            let count = count.clone();
            let gets = get_count.clone();
            async move {
                assert!(request.uri().query().is_none());
                assert_eq!(request.headers()["origin"], UPSTREAM);
                if request.method() == reqwest::Method::POST {
                    assert_eq!(request.uri().path(), "/api/user/sign_in");
                    count.fetch_add(1, Ordering::SeqCst);
                    assert!(
                        axum::body::to_bytes(request.into_body(), 1)
                            .await
                            .unwrap()
                            .is_empty()
                    );
                    return match mode {
                        1 => Json(
                            json!({"success":false,"message":"already checked in password-secret"}),
                        )
                        .into_response(),
                        2 => (StatusCode::INTERNAL_SERVER_ERROR, "oops").into_response(),
                        _ => Json(json!({"success":true,"message":"签到成功"})).into_response(),
                    };
                }
                let n = gets.fetch_add(1, Ordering::SeqCst);
                if mode == 3 {
                    return (StatusCode::UNAUTHORIZED, Json(json!({"success":false})))
                        .into_response();
                }
                if mode == 4 && n == 0 {
                    return (
                        StatusCode::FORBIDDEN,
                        "<html>cf-chl- challenge-platform</html>",
                    )
                        .into_response();
                }
                Json(json!({"success":true,"data":{"id":if mode==5 {8}else{7}}})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            state_path: dir.path().join("private/account.json"),
            ..Config::default()
        };
        let mut app = App::new(
            config,
            Some(crate::tests::active()),
            Upstream::mock(format!("http://{addr}")),
            Arc::new(Provider(verifies.clone())),
        );
        Arc::get_mut(&mut app).unwrap().checkin_clock = fixed;
        (app, posts, verifies, dir, server)
    }
    async fn settled(app: &App) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if app
                    .accounts
                    .lock()
                    .await
                    .operation
                    .as_ref()
                    .is_some_and(|o| o.status != "running")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn http_once_strict_success_expired_waf_and_identity() {
        for mode in 0..6 {
            let (app, posts, verifies, _dir, server) = fixture(mode).await;
            assert_eq!(
                start(&app, Trigger::Manual, false, fixed())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::ACCEPTED
            );
            settled(&app).await;
            assert_eq!(
                posts.load(Ordering::SeqCst),
                usize::from(mode < 3 || mode == 4)
            );
            assert_eq!(verifies.load(Ordering::SeqCst), usize::from(mode == 4));
            let status = app.checkin.lock().await.history[0].status;
            assert!(
                status
                    == match mode {
                        0 | 4 => Status::Success,
                        1 | 2 => Status::Unknown,
                        _ => Status::Failed,
                    }
            );
            if mode == 0 {
                assert_eq!(
                    start(&app, Trigger::Manual, false, fixed())
                        .await
                        .unwrap()
                        .status(),
                    StatusCode::OK
                );
                assert_eq!(posts.load(Ordering::SeqCst), 1);
            }
            if mode == 1 {
                assert_eq!(
                    start(&app, Trigger::Manual, false, fixed())
                        .await
                        .unwrap_err()
                        .1
                        .code,
                    "checkin_retry_confirmation_required"
                );
            }
            app.checkin.lock().await.settings.enabled = true;
            assert_eq!(
                start(&app, Trigger::Scheduled, false, fixed())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
            app.shutdown().await;
            server.abort();
        }
    }
    #[tokio::test]
    async fn no_account_busy_and_intent_failure_have_zero_posts() {
        let (app, posts, _, dir, server) = fixture(0).await;
        {
            let mut a = app.accounts.lock().await;
            app::start(&mut a, "refresh", "reading_account");
        }
        assert_eq!(
            start(&app, Trigger::Manual, false, fixed())
                .await
                .unwrap_err()
                .1
                .code,
            "operation_in_progress"
        );
        app.accounts.lock().await.operation = None;
        std::fs::write(dir.path().join("private"), b"not a directory").unwrap();
        assert_eq!(
            start(&app, Trigger::Manual, false, fixed())
                .await
                .unwrap_err()
                .1
                .code,
            "persistence_failed"
        );
        app.accounts.lock().await.active = None;
        assert_eq!(
            start(&app, Trigger::Manual, false, fixed())
                .await
                .unwrap_err()
                .1
                .code,
            "account_not_configured"
        );
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        app.shutdown().await;
        server.abort();
    }
    #[tokio::test]
    async fn cross_cycle_blocks_both_and_scheduler_shutdown() {
        let (app, posts, _, _dir, server) = fixture(0).await;
        start(
            &app,
            Trigger::Manual,
            false,
            fixed() - chrono::Duration::days(1),
        )
        .await
        .unwrap();
        settled(&app).await;
        let state = app.checkin.lock().await;
        assert_eq!(state.history.len(), 2);
        assert!(state.history.iter().all(|r| r.status == Status::Unknown));
        drop(state);
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        scheduler(&app);
        app.shutdown().await;
        assert!(app.stopping.load(Ordering::SeqCst));
        server.abort();
    }

    #[tokio::test]
    async fn result_persistence_failure_is_unknown_and_never_automatically_retried() {
        let (app, posts, _, _dir, server) = fixture(0).await;
        let mut saved = Saved::default();
        saved.settings.enabled = true;
        saved.put(record());
        let path = app.config.checkin_path();
        save(&path, &saved).unwrap();
        *app.checkin.lock().await = saved;
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let active = app.accounts.lock().await.active.clone().unwrap();
        execute(&app, active, record()).await;
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        assert!(app.checkin.lock().await.history[0].status == Status::Unknown);
        assert_eq!(
            start(&app, Trigger::Scheduled, false, fixed())
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            start(&app, Trigger::Manual, false, fixed())
                .await
                .unwrap_err()
                .1
                .code,
            "checkin_retry_confirmation_required"
        );
        app.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn panicking_preflight_provider_finishes_unknown_without_retry() {
        struct PanicProvider;
        #[async_trait::async_trait]
        impl LoginProvider for PanicProvider {
            async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
                unreachable!()
            }
            async fn verify_session(
                &self,
                _: Credentials,
                _: Duration,
            ) -> Result<Credentials, SafeError> {
                panic!("test preflight panic");
            }
        }
        let (mut app, posts, _, _dir, server) = fixture(4).await;
        Arc::get_mut(&mut app).unwrap().login = Arc::new(PanicProvider);
        app.checkin.lock().await.settings.enabled = true;
        start(&app, Trigger::Manual, false, fixed()).await.unwrap();
        settled(&app).await;
        assert_eq!(
            app.accounts.lock().await.operation.as_ref().unwrap().status,
            "failed"
        );
        assert!(app.checkin.lock().await.history[0].status == Status::Unknown);
        assert!(
            load_at(&app.config.checkin_path(), fixed())
                .unwrap()
                .history[0]
                .status
                == Status::Unknown
        );
        assert_eq!(
            start(&app, Trigger::Scheduled, false, fixed())
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            start(&app, Trigger::Manual, false, fixed())
                .await
                .unwrap_err()
                .1
                .code,
            "checkin_retry_confirmation_required"
        );
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        app.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn final_save_failure_keeps_intent_and_blocks_both_cycles_on_restart() {
        let (app, posts, _, _dir, server) = fixture(0).await;
        app.checkin.lock().await.settings.enabled = true;
        app.checkin_fail_result_save.store(true, Ordering::SeqCst);
        let before = at("2026-10-01T07:59:59+08:00");
        start(&app, Trigger::Manual, false, before).await.unwrap();
        settled(&app).await;
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        let disk: Saved =
            serde_json::from_slice(&std::fs::read(app.config.checkin_path()).unwrap()).unwrap();
        assert_eq!(disk.history.len(), 1);
        assert!(disk.history[0].status == Status::Running);
        let after = at("2026-10-01T08:00:00+08:00");
        *app.checkin.lock().await = load_at(&app.config.checkin_path(), after).unwrap();
        app.checkin_fail_result_save.store(false, Ordering::SeqCst);
        for time in [before, after] {
            assert!(
                app.checkin
                    .lock()
                    .await
                    .find("7", &cycle(time))
                    .unwrap()
                    .status
                    == Status::Unknown
            );
            assert_eq!(
                start(&app, Trigger::Scheduled, false, time)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                start(&app, Trigger::Manual, false, time)
                    .await
                    .unwrap_err()
                    .1
                    .code,
                "checkin_retry_confirmation_required"
            );
        }
        assert_eq!(posts.load(Ordering::SeqCst), 1); // Zero additional POSTs after recovery.
        assert_eq!(
            load_at(&app.config.checkin_path(), after)
                .unwrap()
                .history
                .len(),
            2
        );
        app.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn scheduled_catchup_manual_race_and_account_filter() {
        let (app, posts, _, _dir, server) = fixture(0).await;
        app.checkin.lock().await.settings.enabled = true;
        let (a, b) = tokio::join!(
            start(&app, Trigger::Scheduled, false, fixed()),
            start(&app, Trigger::Manual, false, fixed())
        );
        assert!(a.is_ok());
        assert!(b.is_err());
        settled(&app).await;
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        assert_eq!(status(&app).await["history"].as_array().unwrap().len(), 1);
        let mut changed = crate::tests::active();
        changed.upstream_user_id = "8".into();
        changed.credentials.api_user = "8".into();
        app.accounts.lock().await.active = Some(Arc::new(changed));
        let dto = status(&app).await;
        assert!(dto["history"].as_array().unwrap().is_empty());
        assert!(dto["today"].is_null());
        app.shutdown().await;
        server.abort();
    }
}

fn minutes(time: &str) -> Option<i64> {
    let b = time.as_bytes();
    if b.len() != 5 || b[2] != b':' || ![b[0], b[1], b[3], b[4]].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let h = (b[0] - b'0') as i64 * 10 + (b[1] - b'0') as i64;
    let m = (b[3] - b'0') as i64 * 10 + (b[4] - b'0') as i64;
    (h < 24 && m < 60).then_some((h * 60 + m + 960) % 1440)
}
fn cycle(now: DateTime<Utc>) -> String {
    now.date_naive().to_string()
}
fn due(now: DateTime<Utc>, time: &str) -> DateTime<Utc> {
    now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
        + chrono::Duration::minutes(minutes(time).expect("validated settings"))
}
impl Saved {
    fn find(&self, user: &str, date: &str) -> Option<&Record> {
        self.history
            .iter()
            .find(|r| r.account_user_id == user && r.date == date)
    }
    fn put(&mut self, record: Record) {
        self.history
            .retain(|r| r.account_user_id != record.account_user_id || r.date != record.date);
        self.history.push(record);
        self.history.sort_by(|a, b| b.date.cmp(&a.date));
        self.history.truncate(180);
    }
    fn valid(&self) -> bool {
        let mut keys = std::collections::HashSet::new();
        minutes(&self.settings.time).is_some()
            && self.history.len() <= 180
            && self.history.iter().all(|r| {
                NaiveDate::parse_from_str(&r.date, "%Y-%m-%d")
                    .is_ok_and(|d| d.to_string() == r.date)
                    && crate::model::positive_id(&r.account_user_id)
                    && keys.insert((&r.date, &r.account_user_id))
                    && valid_timestamp(&r.started_at)
                    && r.finished_at.as_ref().is_none_or(|s| valid_timestamp(s))
                    && (r.status == Status::Running) == r.finished_at.is_none()
            })
    }
}
fn valid_timestamp(s: &str) -> bool {
    s.len() <= 40
        && DateTime::parse_from_rfc3339(s).is_ok_and(|t| t.offset().local_minus_utc() == 0)
}
pub fn load(path: &Path) -> Result<Saved, &'static str> {
    load_at(path, Utc::now())
}

fn load_at(path: &Path, now: DateTime<Utc>) -> Result<Saved, &'static str> {
    use std::os::unix::fs::PermissionsExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Saved::default()),
        Err(_) => return Err("Cannot load check-in state."),
    };
    let parent = path
        .parent()
        .and_then(|p| std::fs::symlink_metadata(p).ok())
        .ok_or("Invalid check-in directory.")?;
    if !meta.is_file()
        || meta.len() > 256 * 1024
        || meta.permissions().mode() & 0o777 != 0o600
        || !parent.is_dir()
        || parent.permissions().mode() & 0o777 != 0o700
    {
        return Err("Invalid check-in state permissions or size.");
    }
    let mut saved: Saved =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| "Cannot read check-in state.")?)
            .map_err(|_| "Invalid check-in state.")?;
    if !saved.valid() {
        return Err("Invalid check-in state.");
    }
    let mut recovered = false;
    let mut blocked = Vec::new();
    for r in &mut saved.history {
        if r.status == Status::Running {
            r.status = Status::Unknown;
            r.code = Some(Code::OutcomeUnknown);
            r.finished_at = Some(now.to_rfc3339());
            // A crash cannot establish on which side of a reset the POST ran.
            if r.date != cycle(now) {
                r.code = Some(Code::CycleAmbiguous);
                let mut current = r.clone();
                current.date = cycle(now);
                blocked.push(current);
            }
            recovered = true;
        }
    }
    for record in blocked {
        if saved.find(&record.account_user_id, &record.date).is_none() {
            saved.put(record);
        }
    }
    if recovered {
        save(path, &saved).map_err(|_| "Cannot recover check-in state.")?;
    }
    Ok(saved)
}
fn save(path: &Path, state: &Saved) -> Result<(), SafeError> {
    use std::{
        fs::{self, DirBuilder, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    };
    let mut temp = None;
    let result = (|| -> std::io::Result<()> {
        if !state.valid() {
            return Err(std::io::Error::other("invalid check-in state"));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| std::io::Error::other("private directory required"))?;
        if !parent.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        if !fs::symlink_metadata(parent)?.is_dir() {
            return Err(std::io::Error::other("invalid directory"));
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let tmp = parent.join(format!(".checkin-{}.tmp", crate::model::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        temp = Some(tmp.clone());
        file.write_all(&serde_json::to_vec(state)?)?;
        file.sync_all()?;
        fs::rename(tmp, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Some(tmp) = temp {
        let _ = std::fs::remove_file(tmp);
    }
    result.map_err(|_| SafeError::new("persistence_failed"))
}
async fn status(app: &App) -> Value {
    let accounts = app.accounts.lock().await;
    let saved = app.checkin.lock().await;
    let now = (app.checkin_clock)();
    let date = cycle(now);
    let user = accounts
        .active
        .as_ref()
        .map(|a| a.upstream_user_id.as_str());
    let history: Vec<_> = saved
        .history
        .iter()
        .filter(|r| Some(r.account_user_id.as_str()) == user)
        .collect();
    let today = user.and_then(|u| saved.find(u, &date));
    let next = if saved.settings.enabled && user.is_some() {
        Some(
            if today.is_some() {
                due(now + chrono::Duration::days(1), &saved.settings.time)
            } else {
                due(now, &saved.settings.time).max(now)
            }
            .to_rfc3339(),
        )
    } else {
        None
    };
    json!({"settings":{"enabled":saved.settings.enabled,"time":saved.settings.time,"timezone":"Asia/Shanghai","reset_time":"08:00"},"cycle_date":date,"today":today,"history":history,"next_run_at":next})
}
pub async fn get_status(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    app::require_session(&app, &headers).await?;
    Ok(Json(status(&app).await))
}
pub async fn settings(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    input: Result<Json<Settings>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    app::require_session(&app, &headers).await?;
    let Json(input) = input.map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "bad_request"))?;
    if minutes(&input.time).is_none() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad_request"));
    }
    {
        let mut state = app.checkin.lock().await;
        let mut updated = state.clone();
        updated.settings = input;
        save(&app.config.checkin_path(), &updated)
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        *state = updated;
    }
    Ok(Json(status(&app).await))
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    #[serde(default)]
    confirm_retry: bool,
}
pub async fn run(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    input: Result<Json<RunInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    app::require_session(&app, &headers).await?;
    let Json(input) = input.map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "bad_request"))?;
    start(
        &app,
        Trigger::Manual,
        input.confirm_retry,
        (app.checkin_clock)(),
    )
    .await
}
async fn start(
    app: &Arc<App>,
    trigger: Trigger,
    confirm: bool,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let mut accounts = app.accounts.lock().await;
    app::idle(&accounts)?;
    let active = accounts
        .active
        .clone()
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "account_not_configured"))?;
    let mut saved = app.checkin.lock().await;
    let date = cycle(now);
    let existing = saved.find(&active.upstream_user_id, &date);
    // Never evict a newer/current tuple to make room after wall-clock rollback.
    if existing.is_none()
        && saved.history.len() >= 180
        && saved.history.last().is_some_and(|r| r.date >= date)
    {
        return Err(ApiError::new(StatusCode::CONFLICT, "checkin_history_full"));
    }
    if matches!(trigger, Trigger::Scheduled)
        && (!saved.settings.enabled || now < due(now, &saved.settings.time) || existing.is_some())
    {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    if let Some(r) = existing {
        if matches!(r.status, Status::Success | Status::AlreadyDone) {
            return Ok(Json(json!({"already_recorded":true})).into_response());
        }
        if matches!(r.status, Status::Unknown | Status::Running) && !confirm {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "checkin_retry_confirmation_required",
            ));
        }
    }
    let record = Record {
        date,
        account_user_id: active.upstream_user_id.clone(),
        trigger,
        status: Status::Running,
        started_at: now.to_rfc3339(),
        finished_at: None,
        code: None,
    };
    let mut updated = saved.clone();
    updated.put(record.clone());
    save(&app.config.checkin_path(), &updated)
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    *saved = updated;
    let response = app::start(&mut accounts, "checkin", "checking_in");
    let worker = app.clone();
    app.spawn_job(async move {
        execute(&worker, active, record).await;
    });
    Ok(response)
}
pub fn scheduler(app: &Arc<App>) {
    let worker = app.clone();
    app.spawn_job(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let _ = start(&worker, Trigger::Scheduled, false, (worker.checkin_clock)()).await;
        }
    });
}
async fn request(app: &App, credentials: &Credentials, post: bool) -> Result<Value, SafeError> {
    let path = if post {
        "/api/user/sign_in"
    } else {
        "/api/user/self"
    };
    let headers = crate::upstream::credential_headers(credentials, path)?;
    let response = app
        .upstream
        .client
        .request(
            if post {
                reqwest::Method::POST
            } else {
                reqwest::Method::GET
            },
            format!("{}{path}", app.upstream.base),
        )
        .headers(headers)
        .header("Origin", UPSTREAM)
        .header("Referer", format!("{UPSTREAM}/console"))
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .await
        .map_err(|_| SafeError::new("upstream_unavailable"))?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| SafeError::new("upstream_unavailable"))?;
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err(SafeError::new("upstream_unexpected_response"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !post && crate::hybrid::recognized_waf(&bytes) && !status.is_server_error() {
        return Err(SafeError::new("upstream_challenge"));
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(SafeError::new("upstream_session_expired"));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| SafeError::new("upstream_unexpected_response"))?;
    if !status.is_success()
        || !value.is_object()
        || value.get("success").and_then(Value::as_bool) != Some(true)
    {
        return Err(SafeError::new("upstream_unexpected_response"));
    }
    if !post && identifier(&value["data"]["id"])? != credentials.api_user {
        return Err(SafeError::new("upstream_session_unverified"));
    }
    Ok(value)
}
async fn execute(app: &App, active: Arc<Active>, mut record: Record) {
    use futures_util::FutureExt;
    let mut post_started = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(app.config.helper_timeout);
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout_at(deadline, async {
        let mut credentials = active.credentials.clone();
        match request(app, &credentials, false).await {
            Err(e) if e.code == "upstream_challenge" => {
                credentials = app
                    .login
                    .verify_session(
                        credentials,
                        deadline.saturating_duration_since(tokio::time::Instant::now()),
                    )
                    .await?;
                if credentials.api_user != active.upstream_user_id || !credentials.validate() {
                    return Err(SafeError::new("upstream_session_unverified"));
                }
                request(app, &credentials, false).await?;
                let mut accounts = app.accounts.lock().await;
                if accounts.active.as_ref().is_none_or(|a| {
                    a.revision != active.revision || a.upstream_user_id != active.upstream_user_id
                }) {
                    return Err(SafeError::new("stale_revision"));
                }
                let mut updated = (*active).clone();
                updated.credentials = credentials.clone();
                crate::store::save(&app.config.state_path, &updated)?;
                accounts.active = Some(Arc::new(updated));
            }
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        post_started = true;
        request(app, &credentials, true).await?;
        Ok(())
    }))
    .catch_unwind()
    .await;
    let panicked = outcome.is_err();
    let result = outcome
        .unwrap_or_else(|_| Ok(Err(SafeError::new("checkin_outcome_unknown"))))
        .unwrap_or_else(|_| Err(SafeError::new("upstream_timeout")));
    let now = (app.checkin_clock)();
    record.finished_at = Some(now.to_rfc3339());
    record.status = if result.is_ok() {
        Status::Success
    } else if post_started || panicked {
        Status::Unknown
    } else {
        Status::Failed
    };
    record.code = result.as_ref().err().map(|e| {
        if post_started || panicked {
            Code::OutcomeUnknown
        } else if e.code == "upstream_session_expired" {
            Code::SessionExpired
        } else {
            Code::PreflightFailed
        }
    });
    let mut saved = app.checkin.lock().await;
    if (post_started || panicked) && record.date != cycle(now) {
        record.status = Status::Unknown;
        record.code = Some(Code::CycleAmbiguous);
        let mut blocked = record.clone();
        blocked.date = cycle(now);
        saved.put(blocked);
    }
    saved.put(record.clone());
    let persisted = save_result(app, &saved);
    if persisted.is_err() {
        record.status = Status::Unknown;
        record.code = Some(Code::PersistenceFailed);
        saved.put(record.clone());
    }
    drop(saved);
    app.finish(if record.status == Status::Success {
        Ok(())
    } else {
        Err(SafeError::new(if persisted.is_err() {
            "persistence_failed"
        } else {
            "checkin_outcome_unknown"
        }))
    })
    .await;
}

fn save_result(app: &App, saved: &Saved) -> Result<(), SafeError> {
    #[cfg(test)]
    if app
        .checkin_fail_result_save
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(SafeError::new("persistence_failed"));
    }
    save(&app.config.checkin_path(), saved)
}
