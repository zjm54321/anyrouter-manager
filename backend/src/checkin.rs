//! Conservative single-attempt check-in. UTC midnight is the fixed UTC+8 08:00 boundary.
use crate::{
    app::{self, App},
    error::{ApiError, ErrorSource, SafeError},
    model::{Credentials, Entry},
    upstream::{UPSTREAM, identifier},
};
use axum::{
    Json,
    extract::{Path as AccountPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub time: String,
    #[serde(default = "default_interval")]
    pub interval_minutes: u16,
}
fn default_interval() -> u16 {
    30
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            time: "09:00".into(),
            interval_minutes: 30,
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
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    #[tokio::test]
    async fn legacy_history_with_65_identities_loads_with_one_current_account() {
        let (app, posts, _, _dir, server) = fixture(0).await;
        assert_eq!(app.accounts.lock().await.portfolio.accounts.len(), 1);
        let mut saved = Saved::default();
        for n in 0..180 {
            let mut r = record();
            r.account_user_id = (n % 65 + 1).to_string();
            r.date = cycle(fixed() - chrono::Duration::days(n + 2));
            r.status = Status::Success;
            r.finished_at = Some(fixed().to_rfc3339());
            saved.history.push(r);
        }
        let path = app.config.checkin_path();
        save(&path, &saved).unwrap();
        let mut legacy = serde_json::to_value(&saved).unwrap();
        legacy["settings"]
            .as_object_mut()
            .unwrap()
            .remove("interval_minutes");
        let bytes = serde_json::to_vec(&legacy).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let recovered = load_at(&path, fixed()).unwrap();
        assert_eq!(recovered.settings.interval_minutes, 30);
        assert_eq!(recovered.history.len(), 180);
        assert_eq!(
            recovered
                .history
                .iter()
                .map(|r| &r.account_user_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            65
        );
        assert_eq!(
            serde_json::to_value(&recovered.history).unwrap(),
            serde_json::to_value(&saved.history).unwrap()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        *app.checkin.lock().await = recovered;
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        app.shutdown().await;
        server.abort();
    }

    fn expired_ambiguous_history(count: i64) -> Saved {
        let mut saved = Saved::default();
        for n in 0..count {
            let mut r = record();
            r.date = cycle(fixed() - chrono::Duration::days(n + 2));
            r.status = Status::Unknown;
            r.code = Some(Code::CycleAmbiguous);
            r.finished_at = Some(fixed().to_rfc3339());
            saved.history.push(r);
        }
        saved
    }

    #[test]
    fn expired_ambiguous_history_allows_new_persisted_intent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/checkin.json");
        let mut saved = expired_ambiguous_history(180);
        save(&path, &saved).unwrap();
        saved = load_at(&path, fixed()).unwrap();
        let oldest = cycle(fixed() - chrono::Duration::days(181));
        saved.put_protected(record(), fixed()).unwrap();
        assert_eq!(saved.history.len(), 180);
        assert!(saved.find("7", &oldest).is_none());
        assert!(saved.find("7", &cycle(fixed())).unwrap().status == Status::Running);
        save(&path, &saved).unwrap();
        let recovered = load_at(&path, fixed()).unwrap();
        assert_eq!(recovered.history.len(), 180);
        assert!(recovered.find("7", &cycle(fixed())).unwrap().status == Status::Unknown);
    }

    #[test]
    fn recovery_prunes_expired_ambiguity_but_preserves_both_reset_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/checkin.json");
        let mut saved = expired_ambiguous_history(179);
        let previous = cycle(fixed() - chrono::Duration::days(1));
        let mut interrupted = record();
        interrupted.date = previous.clone();
        saved.history.push(interrupted);
        save(&path, &saved).unwrap();
        let recovered = load_at(&path, fixed()).unwrap();
        assert_eq!(recovered.history.len(), 180);
        assert!(
            recovered
                .find("7", &cycle(fixed() - chrono::Duration::days(180)))
                .is_none()
        );
        for date in [&previous, &cycle(fixed())] {
            let r = recovered.find("7", date).unwrap();
            assert!(r.status == Status::Unknown);
            assert!(r.code == Some(Code::CycleAmbiguous));
        }
        let reloaded = load_at(&path, fixed()).unwrap();
        assert_eq!(
            serde_json::to_value(&reloaded.history).unwrap(),
            serde_json::to_value(&recovered.history).unwrap()
        );
    }

    #[test]
    fn current_previous_future_and_running_history_remain_protected() {
        for anchor in [-1, 0, 1] {
            let mut saved = expired_ambiguous_history(179);
            for r in &mut saved.history {
                r.status = Status::Running;
                r.finished_at = None;
                r.code = None;
            }
            let mut protected = record();
            protected.date = cycle(fixed() + chrono::Duration::days(anchor));
            protected.status = Status::Unknown;
            protected.code = Some(Code::CycleAmbiguous);
            protected.finished_at = Some(fixed().to_rfc3339());
            saved.history.push(protected);
            let before = serde_json::to_vec(&saved).unwrap();
            let mut intent = record();
            if anchor == 0 {
                intent.date = cycle(fixed() - chrono::Duration::days(1));
            }
            assert_eq!(
                saved.put_protected(intent, fixed()).unwrap_err().code,
                "checkin_history_full"
            );
            assert_eq!(serde_json::to_vec(&saved).unwrap(), before);
            // On a backward clock jump even old unknown dates become future,
            // and must remain protected instead of reopening duplicate attempts.
            let mut rollback = expired_ambiguous_history(180);
            let rollback_time = fixed() - chrono::Duration::days(500);
            let mut intent = record();
            intent.date = cycle(rollback_time);
            assert!(rollback.put_protected(intent, rollback_time).is_err());
        }
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
                        7 => (
                            StatusCode::BAD_REQUEST,
                            [("content-encoding", "gzip")],
                            crate::upstream_body::gzip(
                                br#"{"success":false,"message":"\u4eca\u65e5\u5df2\u7b7e\u5230"}"#,
                            ),
                        )
                            .into_response(),
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
                if (mode == 6 && n == 0) || mode == 8 {
                    return (
                        [("content-encoding", "gzip")],
                        crate::upstream_body::gzip(b"<html><script>acw_sc__v2</script></html>"),
                    )
                        .into_response();
                }
                if mode == 9 {
                    return (
                        [("content-encoding", "gzip")],
                        b"invalid-gzip-private-sentinel",
                    )
                        .into_response();
                }
                if mode >= 6 {
                    return (
                        [("content-encoding", "gzip")],
                        crate::upstream_body::gzip(br#"{"success":true,"data":{"id":7}}"#),
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
            crate::model::Portfolio::from_active(crate::tests::active()),
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
            if matches!(mode, 3 | 5) {
                let accounts = app.accounts.lock().await;
                let error = accounts.operation.as_ref().unwrap().error.as_ref().unwrap();
                assert_eq!(
                    error.code,
                    if mode == 3 {
                        "upstream_session_expired"
                    } else {
                        "checkin_preflight_failed"
                    }
                );
                assert!(matches!(error.source, Some(ErrorSource::SelfAccount)));
            }
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

    #[test]
    fn already_done_requires_exact_structured_message_and_appropriate_status() {
        for message in [
            "今日已签到",
            "今天已签到",
            "您今天已经签到过了",
            "今日已经签到",
            " 今日已签到 ",
        ] {
            for status in [StatusCode::OK, StatusCode::BAD_REQUEST] {
                assert!(
                    parse_result(status, &json!({"success":false,"message":message})).unwrap()
                        == Status::AlreadyDone
                );
            }
        }
        for (status, value) in [
            (
                StatusCode::OK,
                json!({"success":false,"message":"签到失败"}),
            ),
            (
                StatusCode::OK,
                json!({"success":false,"message":"今日已签到 private-sentinel"}),
            ),
            (
                StatusCode::OK,
                json!({"success":"false","message":"今日已签到"}),
            ),
            (StatusCode::OK, json!({"message":"今日已签到"})),
            (
                StatusCode::FORBIDDEN,
                json!({"success":false,"message":"今日已签到"}),
            ),
            (StatusCode::INTERNAL_SERVER_ERROR, json!({"success":true})),
        ] {
            let error = parse_result(status, &value).err().unwrap();
            assert_eq!(error.code, "checkin_outcome_unknown");
            assert!(!serde_json::to_string(&error).unwrap().contains("sentinel"));
        }
    }

    #[tokio::test]
    async fn gzip_preflight_bounded_browser_and_already_done_keep_record_classification() {
        for mode in 6..10 {
            let (app, posts, verifies, _dir, server) = fixture(mode).await;
            start(&app, Trigger::Manual, false, fixed()).await.unwrap();
            settled(&app).await;
            assert_eq!(posts.load(Ordering::SeqCst), usize::from(mode < 8));
            assert_eq!(
                verifies.load(Ordering::SeqCst),
                usize::from(matches!(mode, 6 | 8))
            );
            let expected = match mode {
                6 => Status::Success,
                7 => Status::AlreadyDone,
                _ => Status::Failed,
            };
            assert!(app.checkin.lock().await.history[0].status == expected);
            let error = app
                .accounts
                .lock()
                .await
                .operation
                .as_ref()
                .unwrap()
                .error
                .clone();
            if mode < 8 {
                assert!(error.is_none());
                assert_eq!(
                    start(&app, Trigger::Manual, false, fixed())
                        .await
                        .unwrap()
                        .status(),
                    StatusCode::OK
                );
                assert_eq!(posts.load(Ordering::SeqCst), 1);
            } else {
                let error = error.unwrap();
                assert_eq!(error.code, "checkin_preflight_failed");
                assert!(matches!(
                    error.stage,
                    Some(crate::system_log::EventStage::CheckinPreflight)
                ));
            }
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
        app.accounts.lock().await.portfolio.accounts.clear();
        assert_eq!(
            start(&app, Trigger::Manual, false, fixed())
                .await
                .unwrap_err()
                .1
                .code,
            "not_found"
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
        let active = Arc::new(app.accounts.lock().await.portfolio.accounts[0].clone());
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
    async fn panicking_preflight_provider_finishes_failed_without_automatic_retry() {
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
        assert!(app.checkin.lock().await.history[0].status == Status::Failed);
        assert!(
            load_at(&app.config.checkin_path(), fixed())
                .unwrap()
                .history[0]
                .status
                == Status::Failed
        );
        assert_eq!(
            start(&app, Trigger::Scheduled, false, fixed())
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            app.accounts
                .lock()
                .await
                .operation
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .code,
            "checkin_preflight_failed"
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
        assert_eq!(app.checkin.lock().await.history.len(), 1);
        let mut changed = crate::tests::active();
        changed.upstream_user_id = "8".into();
        changed.credentials.api_user = "8".into();
        app.accounts.lock().await.portfolio = crate::model::Portfolio::from_active(changed);
        let dto = status(&app).await;
        assert_eq!(dto["schedule"][0]["status"], "pending");
        app.shutdown().await;
        server.abort();
    }

    #[test]
    fn ordered_slots_interval_overflow_and_per_account_history() {
        let mut settings = Settings {
            enabled: false,
            time: "09:00".into(),
            interval_minutes: 30,
        };
        let now = at("2026-10-01T08:00:00+08:00");
        assert_eq!(slot(now, &settings, 0), at("2026-10-01T09:00:00+08:00"));
        assert_eq!(slot(now, &settings, 1), at("2026-10-01T09:30:00+08:00"));
        assert_eq!(
            slot(at("2026-10-01T07:59:59+08:00"), &settings, 1),
            at("2026-09-30T09:30:00+08:00")
        );
        settings.interval_minutes = 11;
        assert_eq!(slot(now, &settings, 1), at("2026-10-01T09:11:00+08:00"));
        settings.time = "07:59".into();
        assert_eq!(slot(now, &settings, 0), at("2026-10-02T07:59:00+08:00"));
        assert_eq!(
            validate_plan(&settings, 2).unwrap_err().1.code,
            "schedule_overflow"
        );
        settings.interval_minutes = 1440;
        settings.time = "08:00".into();
        assert!(validate_plan(&settings, 1).is_ok());
        assert!(validate_plan(&settings, 2).is_err());
        let mut saved = Saved::default();
        for user in ["7", "8"] {
            for n in 0..200 {
                let mut r = record();
                r.account_user_id = user.into();
                r.date = cycle(fixed() - chrono::Duration::days(200 - n));
                r.status = Status::Success;
                r.finished_at = Some(fixed().to_rfc3339());
                saved.put_protected(r, fixed()).unwrap();
            }
        }
        assert_eq!(saved.history.len(), 360);
        assert!(saved.valid());
        let mut rollback = record();
        rollback.date = cycle(fixed() - chrono::Duration::days(500));
        assert!(
            saved
                .put_protected(rollback, fixed() - chrono::Duration::days(500))
                .is_err()
        );
        // Legacy settings acquire the default interval without losing records.
        let mut legacy = serde_json::to_value(&saved).unwrap();
        legacy["settings"]
            .as_object_mut()
            .unwrap()
            .remove("interval_minutes");
        let migrated: Saved = serde_json::from_value(legacy).unwrap();
        assert_eq!(migrated.settings.interval_minutes, 30);
        assert_eq!(migrated.history.len(), 360);
    }

    #[tokio::test]
    async fn two_account_slots_failure_busy_catchup_and_no_auto_retry() {
        let (app, _, _, _dir, old_server) = fixture(0).await;
        old_server.abort();
        // Both distinct credentials are checked by this loopback-only server.
        let attempts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let captured = attempts.clone();
        let router = Router::new().fallback(any(move |request: axum::extract::Request| {
            let captured = captured.clone();
            async move {
                let user = request.headers()["new-api-user"]
                    .to_str()
                    .unwrap()
                    .to_owned();
                let cookie = request.headers()["cookie"].to_str().unwrap();
                assert_eq!(
                    cookie,
                    if user == "7" {
                        "session=private-upstream-cookie"
                    } else {
                        "session=second-checkin-cookie"
                    }
                );
                if request.method() == reqwest::Method::POST {
                    captured.lock().unwrap().push(user);
                    Json(json!({"success":true})).into_response()
                } else if user == "7" {
                    (StatusCode::UNAUTHORIZED, Json(json!({"success":false}))).into_response()
                } else {
                    Json(json!({"success":true,"data":{"id":8}})).into_response()
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        // Move into a new App to inject the local transport, never mutate production origin.
        let mut portfolio = app.accounts.lock().await.portfolio.clone();
        let mut b = portfolio.accounts[0].clone();
        b.id = crate::model::id();
        b.upstream_user_id = "8".into();
        b.credentials.api_user = "8".into();
        b.credentials.cookies[0].value = "second-checkin-cookie".into();
        b.selected_key = None;
        let id_a = portfolio.accounts[0].id.clone();
        let id_b = b.id.clone();
        portfolio.accounts.push(b);
        let upstream = Upstream::mock(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let config = Config {
            state_path: app.config.state_path.clone(),
            ..Config::default()
        };
        let mut app = App::new(config, portfolio, upstream, app.login.clone());
        Arc::get_mut(&mut app).unwrap().checkin_clock = fixed;
        app.checkin.lock().await.settings.enabled = true;
        let nine = at("2026-10-01T09:00:00+08:00");
        scheduler_tick(&app, nine - chrono::Duration::seconds(1)).await;
        assert!(app.accounts.lock().await.operation.is_none());
        scheduler_tick(&app, nine).await;
        settled(&app).await;
        assert!(
            app.checkin
                .lock()
                .await
                .find("7", &cycle(nine))
                .unwrap()
                .status
                == Status::Failed
        );
        assert_eq!(
            app.accounts
                .lock()
                .await
                .operation
                .as_ref()
                .unwrap()
                .account_id
                .as_deref(),
            Some(id_a.as_str())
        );
        scheduler_tick(&app, nine + chrono::Duration::minutes(29)).await;
        assert!(attempts.lock().unwrap().is_empty());
        {
            let mut a = app.accounts.lock().await;
            app::start(&mut a, "refresh", "reading_account");
        }
        scheduler_tick(&app, nine + chrono::Duration::minutes(30)).await;
        assert!(app.checkin.lock().await.find("8", &cycle(nine)).is_none());
        app.accounts.lock().await.operation = None;
        scheduler_tick(&app, nine + chrono::Duration::minutes(31)).await;
        settled(&app).await;
        assert_eq!(*attempts.lock().unwrap(), vec!["8"]);
        assert_eq!(
            app.accounts
                .lock()
                .await
                .operation
                .as_ref()
                .unwrap()
                .account_id
                .as_deref(),
            Some(id_b.as_str())
        );
        scheduler_tick(&app, nine + chrono::Duration::hours(1)).await;
        assert_eq!(*attempts.lock().unwrap(), vec!["8"]);
        // Restart turns durable running B into unknown; A failure remains independent.
        let mut saved = app.checkin.lock().await.clone();
        let mut r = record();
        r.account_user_id = "8".into();
        saved.put_protected(r, fixed()).unwrap();
        save(&app.config.checkin_path(), &saved).unwrap();
        *app.checkin.lock().await = load_at(&app.config.checkin_path(), fixed()).unwrap();
        scheduler_tick(&app, fixed()).await;
        assert_eq!(*attempts.lock().unwrap(), vec!["8"]);
        assert_eq!(
            start_account(&app, &id_b, Trigger::Manual, false, fixed())
                .await
                .unwrap_err()
                .1
                .code,
            "checkin_retry_confirmation_required"
        );
        // Adding an account into an already elapsed slot gets exactly one catch-up.
        let mut c = app.accounts.lock().await.portfolio.accounts[1].clone();
        c.id = crate::model::id();
        c.upstream_user_id = "9".into();
        c.credentials.api_user = "9".into();
        // The fixture deliberately rejects ID 9 as an identity mismatch: one failed
        // attempt still blocks automatic retry without changing any other slot.
        app.accounts.lock().await.portfolio.accounts.push(c);
        scheduler_tick(&app, fixed()).await;
        settled(&app).await;
        assert!(
            app.checkin
                .lock()
                .await
                .find("9", &cycle(fixed()))
                .unwrap()
                .status
                == Status::Failed
        );
        scheduler_tick(&app, fixed()).await;
        assert_eq!(*attempts.lock().unwrap(), vec!["8"]);
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
pub(crate) fn validate_plan(settings: &Settings, count: usize) -> Result<(), ApiError> {
    let offset = minutes(&settings.time)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "bad_request"))?;
    if !(1..=1440).contains(&settings.interval_minutes) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad_request"));
    }
    if count > 0 && offset + (count - 1) as i64 * i64::from(settings.interval_minutes) >= 1440 {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "schedule_overflow",
        ));
    }
    Ok(())
}
fn slot(now: DateTime<Utc>, settings: &Settings, index: usize) -> DateTime<Utc> {
    due(now, &settings.time)
        + chrono::Duration::minutes(index as i64 * i64::from(settings.interval_minutes))
}
impl Saved {
    fn find(&self, user: &str, date: &str) -> Option<&Record> {
        self.history
            .iter()
            .find(|r| r.account_user_id == user && r.date == date)
    }
    #[cfg(test)]
    fn put(&mut self, record: Record) {
        // Keep a separate bounded history for each identity, not one shared 180-row ring.
        let user = record.account_user_id.clone();
        self.history
            .retain(|r| r.account_user_id != record.account_user_id || r.date != record.date);
        self.history.push(record);
        self.history.sort_by(|a, b| b.date.cmp(&a.date));
        let mut n = 0;
        self.history.retain(|r| {
            if r.account_user_id != user {
                true
            } else {
                n += 1;
                n <= 180
            }
        });
    }
    fn put_protected(&mut self, record: Record, now: DateTime<Utc>) -> Result<(), SafeError> {
        let user = record.account_user_id.clone();
        let date = record.date.clone();
        let previous = cycle(now - chrono::Duration::days(1));
        let mut history = self.history.clone();
        history.retain(|r| r.account_user_id != user || r.date != date);
        history.push(record);
        history.sort_by(|a, b| b.date.cmp(&a.date));
        while history.iter().filter(|r| r.account_user_id == user).count() > 180 {
            let index = history
                .iter()
                .rposition(|r| {
                    r.account_user_id == user
                        && r.date < previous
                        && r.date != date
                        && r.status != Status::Running
                })
                .ok_or_else(|| SafeError::new("checkin_history_full"))?;
            history.remove(index);
        }
        if history.len() > 64 * 180 {
            return Err(SafeError::new("checkin_history_full"));
        }
        self.history = history;
        Ok(())
    }
    fn valid(&self) -> bool {
        let mut keys = std::collections::HashSet::new();
        let mut counts = std::collections::HashMap::new();
        minutes(&self.settings.time).is_some()
            && (1..=1440).contains(&self.settings.interval_minutes)
            && self.history.len() <= 64 * 180
            && self.history.iter().all(|r| {
                let count = counts.entry(&r.account_user_id).or_insert(0usize);
                *count += 1;
                NaiveDate::parse_from_str(&r.date, "%Y-%m-%d")
                    .is_ok_and(|d| d.to_string() == r.date)
                    && crate::model::positive_id(&r.account_user_id)
                    && keys.insert((&r.date, &r.account_user_id))
                    && valid_timestamp(&r.started_at)
                    && r.finished_at.as_ref().is_none_or(|s| valid_timestamp(s))
                    && (r.status == Status::Running) == r.finished_at.is_none()
                    && *count <= 180
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
        || meta.len() > 8 * 1024 * 1024
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
            saved
                .put_protected(record, now)
                .map_err(|_| "Cannot safely recover check-in history.")?;
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
        let bytes = serde_json::to_vec(state)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(std::io::Error::other("state too large"));
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        let directory = fs::File::open(parent)?;
        directory.sync_all()?;
        fs::rename(tmp, path)?;
        let _ = directory.sync_all();
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
    let schedule: Vec<_> = accounts.portfolio.accounts.iter().enumerate().map(|(i,a)| json!({"account_id":a.id,"scheduled_at":slot(now,&saved.settings,i).to_rfc3339(),"status":saved.find(&a.upstream_user_id,&date).map(|r|serde_json::to_value(r.status).unwrap()).unwrap_or(json!("pending"))})).collect();
    let next = if saved.settings.enabled {
        accounts
            .portfolio
            .accounts
            .iter()
            .enumerate()
            .map(|(i, a)| next_for(&saved, a, now, i))
            .min()
            .map(|n| n.to_rfc3339())
    } else {
        None
    };
    json!({"settings":{"enabled":saved.settings.enabled,"time":saved.settings.time,"interval_minutes":saved.settings.interval_minutes,"timezone":"Asia/Shanghai","reset_time":"08:00"},"cycle_date":date,"schedule":schedule,"next_run_at":next})
}
fn next_for(saved: &Saved, account: &Entry, now: DateTime<Utc>, index: usize) -> DateTime<Utc> {
    if saved.find(&account.upstream_user_id, &cycle(now)).is_some() {
        slot(now + chrono::Duration::days(1), &saved.settings, index)
    } else {
        slot(now, &saved.settings, index).max(now)
    }
}
pub async fn get_account_status(
    State(app): State<Arc<App>>,
    AccountPath(id): AccountPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    app::require_session(&app, &headers).await?;
    let accounts = app.accounts.lock().await;
    let account = app::entry(&accounts, &id)?;
    let index = accounts
        .portfolio
        .accounts
        .iter()
        .position(|a| a.id == id)
        .unwrap();
    let saved = app.checkin.lock().await;
    let now = (app.checkin_clock)();
    let date = cycle(now);
    let history: Vec<_> = saved
        .history
        .iter()
        .filter(|r| r.account_user_id == account.upstream_user_id)
        .collect();
    Ok(Json(
        json!({"account_id":id,"cycle_date":date,"today":saved.find(&account.upstream_user_id,&date),"history":history,"next_run_at":saved.settings.enabled.then(||next_for(&saved,account,now,index).to_rfc3339())}),
    ))
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
    input: Result<Json<SettingsInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    app::require_session(&app, &headers).await?;
    let Json(input) = input.map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "bad_request"))?;
    let input = Settings {
        enabled: input.enabled,
        time: input.time,
        interval_minutes: input.interval_minutes,
    };
    {
        let accounts = app.accounts.lock().await;
        app::idle(&accounts)?;
        validate_plan(&input, accounts.portfolio.accounts.len())?;
        let mut state = app.checkin.lock().await;
        let mut updated = state.clone();
        updated.settings = input;
        save(&app.config.checkin_path(), &updated)
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        *state = updated;
    }
    Ok(Json(status(&app).await))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsInput {
    enabled: bool,
    time: String,
    interval_minutes: u16,
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RunInput {
    #[serde(default)]
    confirm_retry: bool,
}
pub async fn run(
    State(app): State<Arc<App>>,
    AccountPath(id): AccountPath<String>,
    headers: HeaderMap,
    input: Result<Json<RunInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    app::require_session(&app, &headers).await?;
    let Json(input) = input.map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "bad_request"))?;
    start_account(
        &app,
        &id,
        Trigger::Manual,
        input.confirm_retry,
        (app.checkin_clock)(),
    )
    .await
}
async fn start_account(
    app: &Arc<App>,
    id: &str,
    trigger: Trigger,
    confirm: bool,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let mut accounts = app.accounts.lock().await;
    app::idle(&accounts)?;
    let active = Arc::new(app::entry(&accounts, id)?.clone());
    let index = accounts
        .portfolio
        .accounts
        .iter()
        .position(|a| a.id == id)
        .unwrap();
    let mut saved = app.checkin.lock().await;
    let date = cycle(now);
    let existing = saved.find(&active.upstream_user_id, &date);
    // Never evict a newer/current tuple to make room after wall-clock rollback.
    if existing.is_none()
        && saved
            .history
            .iter()
            .filter(|r| r.account_user_id == active.upstream_user_id)
            .count()
            >= 180
        && saved
            .history
            .iter()
            .rev()
            .find(|r| r.account_user_id == active.upstream_user_id)
            .is_some_and(|r| r.date >= date || r.date >= cycle(now - chrono::Duration::days(1)))
    {
        return Err(ApiError::new(StatusCode::CONFLICT, "checkin_history_full"));
    }
    if matches!(trigger, Trigger::Scheduled)
        && (!saved.settings.enabled
            || now < slot(now, &saved.settings, index)
            || existing.is_some())
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
    updated
        .put_protected(record.clone(), now)
        .map_err(|e| ApiError(StatusCode::CONFLICT, e))?;
    save(&app.config.checkin_path(), &updated)
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    *saved = updated;
    let response = app::start_for(&mut accounts, "checkin", "checking_in", Some(id.into()));
    drop(saved);
    drop(accounts);
    let context = app.operation_context().await;
    let worker = app.clone();
    app.spawn_operation(context, async move {
        crate::logging::emit(
            crate::log_settings::LogLevel::Info,
            crate::system_log::EventKind::CheckinStart,
            crate::system_log::EventStage::Checkin,
            None,
            None,
        );
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
            scheduler_tick(&worker, (worker.checkin_clock)()).await;
        }
    });
}
async fn scheduler_tick(app: &Arc<App>, now: DateTime<Utc>) {
    let ids: Vec<_> = app
        .accounts
        .lock()
        .await
        .portfolio
        .accounts
        .iter()
        .map(|a| a.id.clone())
        .collect();
    for id in ids {
        match start_account(app, &id, Trigger::Scheduled, false, now).await {
            Ok(response) if response.status() == StatusCode::ACCEPTED => break,
            Err(e) if e.1.code == "operation_in_progress" => break,
            _ => {}
        }
    }
}
#[cfg(test)]
async fn start(
    app: &Arc<App>,
    trigger: Trigger,
    confirm: bool,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let id = app
        .accounts
        .lock()
        .await
        .portfolio
        .accounts
        .first()
        .map(|a| a.id.clone())
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found"))?;
    start_account(app, &id, trigger, confirm, now).await
}
async fn request(
    app: &App,
    credentials: &Credentials,
    post: bool,
) -> Result<Option<Status>, SafeError> {
    use crate::system_log::{EventReason, EventStage};
    let stage = if post {
        EventStage::Checkin
    } else {
        EventStage::CheckinPreflight
    };
    let result = request_inner(app, credentials, post, stage).await;
    result.map_err(|mut e| {
        e.stage = Some(stage);
        if !post {
            e.source = Some(ErrorSource::SelfAccount);
        }
        if e.reason.is_none() && e.code == "upstream_unexpected_response" {
            e.reason = Some(EventReason::SchemaInvalid);
        }
        e
    })
}
async fn request_inner(
    app: &App,
    credentials: &Credentials,
    post: bool,
    stage: crate::system_log::EventStage,
) -> Result<Option<Status>, SafeError> {
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
        .map_err(|e| {
            SafeError::new(if e.is_timeout() {
                "upstream_timeout"
            } else {
                "upstream_unavailable"
            })
        })?;
    let status = response.status();
    crate::logging::emit(
        crate::log_settings::LogLevel::Debug,
        if post {
            crate::system_log::EventKind::CheckinStart
        } else {
            crate::system_log::EventKind::StorageRead
        },
        stage,
        None,
        Some(status.as_u16()),
    );
    let (_, value) = crate::upstream_body::json(response)
        .await
        .map_err(|e| e.at(stage, Some(status.as_u16())))?;
    if post {
        return parse_result(status, &value)
            .map(Some)
            .map_err(|e| e.at(stage, Some(status.as_u16())));
    }
    let data = crate::upstream_body::data(status, value)
        .map_err(|e| e.at(stage, Some(status.as_u16())))?;
    if !identifier(&data["id"]).is_ok_and(|id| id == credentials.api_user) {
        return Err(SafeError::new("upstream_session_unverified").at(stage, Some(status.as_u16())));
    }
    Ok(None)
}

fn parse_result(status: StatusCode, value: &Value) -> Result<Status, SafeError> {
    if status.is_success()
        && value.is_object()
        && value.get("success").and_then(Value::as_bool) == Some(true)
    {
        return Ok(Status::Success);
    }
    // Conservative exact compatibility phrases, not evidence from a live response.
    // No substring matching, translation guessing, or acceptance of a generic failure.
    if (status.is_success() || status == StatusCode::BAD_REQUEST)
        && value.is_object()
        && value.get("success").and_then(Value::as_bool) == Some(false)
        && value
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| {
                matches!(
                    message.trim(),
                    "今日已签到" | "今天已签到" | "您今天已经签到过了" | "今日已经签到"
                )
            })
    {
        return Ok(Status::AlreadyDone);
    }
    Err(SafeError::new("checkin_outcome_unknown")
        .with_reason(crate::system_log::EventReason::OutcomeUnknown))
}
async fn execute(app: &App, active: Arc<Entry>, mut record: Record) {
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
                    .await
                    .map_err(|e| {
                        e.with_source(ErrorSource::Helper)
                            .at(crate::system_log::EventStage::CheckinPreflight, None)
                    })?;
                if credentials.api_user != active.upstream_user_id || !credentials.validate() {
                    return Err(SafeError::new("upstream_session_unverified"));
                }
                request(app, &credentials, false).await?;
                let mut accounts = app.accounts.lock().await;
                if accounts
                    .portfolio
                    .accounts
                    .iter()
                    .find(|a| a.id == active.id)
                    .is_none_or(|a| {
                        a.revision != active.revision
                            || a.upstream_user_id != active.upstream_user_id
                    })
                {
                    return Err(SafeError::new("stale_revision"));
                }
                let mut updated = (*active).clone();
                updated.credentials = credentials.clone();
                app::replace_entry(app, &mut accounts, updated)?;
            }
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        post_started = true;
        request(app, &credentials, true)
            .await
            .map(|status| status.expect("POST result"))
    }))
    .catch_unwind()
    .await;
    let result = outcome
        .unwrap_or_else(|_| Ok(Err(SafeError::new("checkin_outcome_unknown"))))
        .unwrap_or_else(|_| Err(SafeError::new("upstream_timeout")));
    let now = (app.checkin_clock)();
    record.finished_at = Some(now.to_rfc3339());
    record.status = if let Ok(status) = &result {
        *status
    } else if post_started {
        Status::Unknown
    } else {
        Status::Failed
    };
    record.code = result.as_ref().err().map(|e| {
        if post_started {
            Code::OutcomeUnknown
        } else if e.code == "upstream_session_expired" {
            Code::SessionExpired
        } else if e.code == "persistence_failed" {
            Code::PersistenceFailed
        } else {
            Code::PreflightFailed
        }
    });
    let mut saved = app.checkin.lock().await;
    if post_started && record.date != cycle(now) {
        record.status = Status::Unknown;
        record.code = Some(Code::CycleAmbiguous);
        let mut blocked = record.clone();
        blocked.date = cycle(now);
        // On an unsafe prune keep all dedupe tuples in memory and the durable running
        // intent on disk; never forget a possibly completed side effect.
        if saved.put_protected(blocked.clone(), now).is_err() {
            saved.history.push(blocked);
        }
    }
    if saved.put_protected(record.clone(), now).is_err() {
        saved.history.push(record.clone());
    }
    let persisted = save_result(app, &saved);
    if persisted.is_err() {
        record.status = Status::Unknown;
        record.code = Some(Code::PersistenceFailed);
        if let Some(r) = saved
            .history
            .iter_mut()
            .find(|r| r.date == record.date && r.account_user_id == record.account_user_id)
        {
            *r = record.clone();
        }
    }
    drop(saved);
    let completed = matches!(record.status, Status::Success | Status::AlreadyDone);
    let operation_result = if completed {
        Ok(())
    } else {
        let code = match record.code {
            Some(Code::SessionExpired) => "upstream_session_expired",
            Some(Code::PreflightFailed) => "checkin_preflight_failed",
            Some(Code::PersistenceFailed) => "persistence_failed",
            Some(Code::CycleAmbiguous) => "checkin_cycle_ambiguous",
            _ => "checkin_outcome_unknown",
        };
        let mut error = SafeError::new(code);
        if code == "persistence_failed" {
            error.source = Some(ErrorSource::Persistence);
        } else if !post_started {
            if let Err(cause) = &result {
                error.source = cause.source;
                error.stage = cause.stage;
                error.reason = cause.reason;
                error.http_status = cause.http_status;
            }
            error
                .stage
                .get_or_insert(crate::system_log::EventStage::CheckinPreflight);
        } else {
            error.stage = Some(crate::system_log::EventStage::Checkin);
            error.http_status = result.as_ref().err().and_then(|cause| cause.http_status);
        }
        Err(error)
    };
    let (stage, reason) = operation_result
        .as_ref()
        .err()
        .map(SafeError::log_context)
        .unwrap_or((
            crate::system_log::EventStage::Checkin,
            if record.status == Status::AlreadyDone {
                crate::system_log::EventReason::AlreadyDone
            } else {
                crate::system_log::EventReason::Success
            },
        ));
    crate::logging::emit(
        if persisted.is_err() {
            crate::log_settings::LogLevel::Error
        } else if completed {
            crate::log_settings::LogLevel::Info
        } else {
            crate::log_settings::LogLevel::Warn
        },
        crate::system_log::EventKind::CheckinFinish,
        stage,
        Some(reason),
        operation_result.as_ref().err().and_then(|e| e.http_status),
    );
    app.finish(operation_result).await;
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
