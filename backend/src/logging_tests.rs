use crate::{
    app::{self, App},
    config::Config,
    error::SafeError,
    helper::LoginProvider,
    log_settings::{LogLevel, LogSettings, SharedLogSettings},
    logging,
    model::{Credentials, Portfolio},
    system_log::{EventKind, EventStage, SystemEvent, SystemLogQuery, SystemLogSink},
    upstream::Upstream,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;
const ROOT: &str = "Fake-Local-Root-51c7e8aa-2345-SufficientEntropy";
struct Local;

#[tokio::test]
async fn refresh_failure_summary_records_actual_source_stage_without_helper_claims() {
    use crate::{error::ErrorSource, system_log::EventReason};
    let (_dir, app) = fixture().await;
    for (source, reason, expected_stage, http_status) in [
        (
            ErrorSource::SelfAccount,
            EventReason::KnownChallenge,
            EventStage::SelfAccount,
            Some(200),
        ),
        (
            ErrorSource::Tokens,
            EventReason::JsonRejected,
            EventStage::Tokens,
            Some(200),
        ),
        (
            ErrorSource::Persistence,
            EventReason::StorageUnavailable,
            EventStage::Storage,
            None,
        ),
    ] {
        {
            let mut state = app.accounts.lock().await;
            app::start_for(
                &mut state,
                "refresh",
                "reading_account",
                Some(crate::model::id()),
            );
        }
        let context = app.operation_context().await.unwrap();
        let operation = app
            .accounts
            .lock()
            .await
            .operation
            .as_ref()
            .unwrap()
            .id
            .clone();
        let error = SafeError::new("upstream_unexpected_response")
            .with_source(source)
            .with_reason(reason)
            .at(expected_stage, http_status);
        logging::CONTEXT
            .scope(context, app.finish(Err(error)))
            .await;
        let sink = app.system_logs.as_ref().unwrap();
        sink.flush().await.unwrap();
        let page = sink
            .page(SystemLogQuery {
                operation_id: Some(operation),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.items.len(), 2);
        for event in &page.items {
            assert!(matches!(
                event.event,
                EventKind::AccountRefresh | EventKind::TaskFinish
            ));
            assert_eq!(event.stage, Some(expected_stage));
            assert_eq!(event.reason, Some(reason));
            assert_eq!(event.http_status, http_status);
            assert!(event.account_id.is_some());
        }
        let encoded = serde_json::to_string(&page).unwrap();
        assert!(!encoded.contains("helper_failed"));
    }
    app.shutdown().await;
    app.shutdown_logs().await;
}

#[tokio::test]
async fn actual_loopback_management_routes_are_private_and_hot() {
    let (_dir, app) = fixture().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let router = app::router(app.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let url = |path: &str| format!("http://{address}{path}");
    let response = client
        .get(url("/api/system-logs"))
        .header("host", "127.0.0.1:8080")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let cookie = session(&app).await;
    let request = |method, path: &str| {
        client
            .request(method, url(path))
            .header("host", "127.0.0.1:8080")
            .header("origin", "http://127.0.0.1:8080")
            .header("cookie", &cookie)
    };
    let settings = json!({"level":"trace","system_retention_days":3,"request_retention_days":4});
    let response = request(reqwest::Method::PUT, "/api/log-settings")
        .json(&settings)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.json::<Value>().await.unwrap(), settings);
    assert_eq!(
        app.log_settings.as_ref().unwrap().snapshot().level,
        LogLevel::Trace
    );
    for path in [
        "/api/system-logs?limit=1001",
        "/api/system-logs?level=info&level=debug",
        "/api/system-logs?password=fake-sentinel",
    ] {
        let response = request(reqwest::Method::GET, path).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response.text().await.unwrap().contains("sentinel"));
    }
    for path in ["/api/system-logs/clear", "/api/request-logs/clear"] {
        let response = request(reqwest::Method::POST, path).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    app.shutdown().await;
    app.shutdown_logs().await;
}

#[async_trait::async_trait]
impl LoginProvider for Local {
    async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
        if logging::diagnostics_enabled() {
            logging::helper_snapshot(Some(
                serde_json::from_value(crate::diagnostics::fixture()).unwrap(),
            ));
        }
        Err(SafeError::new("upstream_login_failed"))
    }
}

#[tokio::test]
async fn next_login_task_uses_hot_diagnostics_without_exposing_api_metadata() {
    let (_dir, app) = fixture().await;
    let cookie = session(&app).await;
    for level in [LogLevel::Info, LogLevel::Debug] {
        app.log_settings
            .as_ref()
            .unwrap()
            .update(LogSettings {
                level,
                system_retention_days: 7,
                request_retention_days: 7,
            })
            .await
            .unwrap();
        let response = call(
            &app,
            "POST",
            "/api/accounts/login",
            Some(&cookie),
            "http://127.0.0.1:8080",
            Some(json!({"username":"private-user-sentinel","password":"fake-password-sentinel"})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let operation = value(response).await["operation_id"]
            .as_str()
            .unwrap()
            .to_owned();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if app.accounts.lock().await.operation.as_ref().unwrap().status != "running" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            app.accounts
                .lock()
                .await
                .operation
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .diagnostics
                .is_none()
        );
        let sink = app.system_logs.as_ref().unwrap();
        sink.flush().await.unwrap();
        let page = sink
            .page(SystemLogQuery {
                operation_id: Some(operation),
                ..Default::default()
            })
            .unwrap();
        assert!(page.items.iter().any(|e| e.event == EventKind::TaskStart));
        assert!(page.items.iter().any(|e| e.event == EventKind::TaskFinish));
        assert_eq!(
            page.items.iter().any(|e| e.diagnostics.is_some()),
            level == LogLevel::Debug
        );
        let text = serde_json::to_string(&page).unwrap();
        assert!(!text.contains("sentinel"));
    }
    app.shutdown().await;
    app.shutdown_logs().await;
}
async fn fixture() -> (tempfile::TempDir, Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        root_key: ROOT.into(),
        state_path: dir.path().join("private/accounts.json"),
        ..Default::default()
    };
    let settings = SharedLogSettings::open(config.log_settings_path())
        .await
        .unwrap();
    let system = SystemLogSink::open(config.system_log_path(), settings.clone())
        .await
        .unwrap();
    let request =
        crate::request_log::LogSink::open_with_retention_days(config.request_log_path(), 7)
            .await
            .unwrap();
    let upstream = Upstream {
        client: reqwest::Client::new(),
        base: "http://127.0.0.1:1".into(),
    };
    let gateway_settings =
        crate::gateway_settings::SharedGatewaySettings::open(config.gateway_settings_path())
            .await
            .unwrap();
    (
        dir,
        App::new_with_system_logs(
            config,
            Portfolio::default(),
            upstream,
            Arc::new(Local),
            Some(request),
            Some(settings),
            Some(system),
            gateway_settings,
        ),
    )
}
async fn call(
    app: &Arc<App>,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    origin: &str,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if path == "/api/admin/session" {
        request = request.header("authorization", format!("Bearer {ROOT}"));
    }
    let body = if let Some(body) = body {
        request = request.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let result = app::router(app.clone())
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    assert_eq!(result.headers()["cache-control"], "no-store");
    result
}
async fn value(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
async fn session(app: &Arc<App>) -> String {
    let response = call(
        app,
        "POST",
        "/api/admin/session",
        None,
        "http://127.0.0.1:8080",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}
#[tokio::test]
async fn management_auth_settings_filters_clear_and_persistence() {
    let (_dir, app) = fixture().await;
    for (method, path) in [
        ("GET", "/api/log-settings"),
        ("PUT", "/api/log-settings"),
        ("GET", "/api/system-logs"),
        ("POST", "/api/system-logs/clear"),
        ("POST", "/api/request-logs/clear"),
    ] {
        assert_eq!(
            call(&app, method, path, None, "http://127.0.0.1:8080", None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let cookie = session(&app).await;
    assert_eq!(
        value(
            call(
                &app,
                "GET",
                "/api/log-settings",
                Some(&cookie),
                "http://127.0.0.1:8080",
                None
            )
            .await
        )
        .await,
        json!({"level":"info","system_retention_days":7,"request_retention_days":7})
    );
    for level in ["error", "warn", "info", "debug", "trace"] {
        let setting = json!({"level":level,"system_retention_days":1,"request_retention_days":2});
        assert_eq!(
            value(
                call(
                    &app,
                    "PUT",
                    "/api/log-settings",
                    Some(&cookie),
                    "http://127.0.0.1:8080",
                    Some(setting.clone())
                )
                .await
            )
            .await,
            setting
        );
    }
    for body in [
        json!({"level":"unknown","system_retention_days":1,"request_retention_days":2}),
        json!({"level":"info","system_retention_days":0,"request_retention_days":2}),
        json!({"level":"info","system_retention_days":1,"request_retention_days":91}),
        json!({"level":"info","system_retention_days":1,"request_retention_days":1.5}),
        json!({"level":"info","system_retention_days":1}),
        json!({"level":"info","system_retention_days":1,"request_retention_days":2,"password":"sentinel"}),
    ] {
        assert_eq!(
            call(
                &app,
                "PUT",
                "/api/log-settings",
                Some(&cookie),
                "http://127.0.0.1:8080",
                Some(body)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/log-settings",
            Some(&cookie),
            "https://evil.invalid",
            Some(json!({}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for query in [
        "limit=0",
        "limit=1001",
        "limit=1.5",
        "limit=1&limit=2",
        "unknown=x",
        "level=verbose",
        "account_id=bad",
        "operation_id=bad",
        "&limit=1",
        "limit=1&",
    ] {
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/api/system-logs?{query}"),
                Some(&cookie),
                "http://127.0.0.1:8080",
                None
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let sink = app.system_logs.as_ref().unwrap();
    sink.emit(SystemEvent::new(LogLevel::Error, EventKind::Timeout));
    sink.emit(SystemEvent::new(LogLevel::Trace, EventKind::TaskStart));
    sink.flush().await.unwrap();
    let rows = value(
        call(
            &app,
            "GET",
            "/api/system-logs?level=error",
            Some(&cookie),
            "http://127.0.0.1:8080",
            None,
        )
        .await,
    )
    .await;
    assert!(
        rows["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["level"] == "error")
    );
    let old = sink
        .page(SystemLogQuery::default())
        .unwrap()
        .items
        .into_iter()
        .map(|e| e.id)
        .collect::<Vec<_>>();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/system-logs/clear",
            Some(&cookie),
            "http://127.0.0.1:8080",
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    sink.flush().await.unwrap();
    assert!(
        sink.page(SystemLogQuery::default())
            .unwrap()
            .items
            .iter()
            .all(|e| !old.contains(&e.id))
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/request-logs/clear",
            Some(&cookie),
            "http://127.0.0.1:8080",
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert!(app.accounts.lock().await.portfolio.accounts.is_empty());
    let reloaded = SharedLogSettings::open(app.config.log_settings_path())
        .await
        .unwrap();
    assert_eq!(reloaded.snapshot().level, LogLevel::Trace);
    assert_eq!(reloaded.snapshot().request_retention_days, 2);
    app.shutdown_logs().await;
}
#[tokio::test]
async fn io_failure_keeps_old_settings_and_clear_is_503_safe() {
    let (_dir, app) = fixture().await;
    let cookie = session(&app).await;
    std::fs::remove_file(app.config.log_settings_path()).unwrap();
    std::fs::create_dir(app.config.log_settings_path()).unwrap();
    let response = call(
        &app,
        "PUT",
        "/api/log-settings",
        Some(&cookie),
        "http://127.0.0.1:8080",
        Some(json!({"level":"trace","system_retention_days":1,"request_retention_days":2})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!value(response).await.to_string().contains("sentinel"));
    assert_eq!(
        app.log_settings.as_ref().unwrap().snapshot().level,
        LogLevel::Info
    );
    std::fs::remove_file(app.config.system_log_path()).unwrap();
    std::fs::create_dir(app.config.system_log_path()).unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/system-logs/clear",
            Some(&cookie),
            "http://127.0.0.1:8080",
            None
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.shutdown_logs().await;
}
#[tokio::test]
async fn task_context_hot_policy_correlation_queue_pressure_and_privacy() {
    let (_dir, app) = fixture().await;
    let sink = app.system_logs.as_ref().unwrap();
    let operation = crate::model::id();
    let info = logging::Context::new(sink.clone(), Some(operation.clone()), None, false);
    app.log_settings
        .as_ref()
        .unwrap()
        .update(LogSettings {
            level: LogLevel::Trace,
            system_retention_days: 7,
            request_retention_days: 7,
        })
        .await
        .unwrap();
    logging::CONTEXT
        .scope(info, async {
            assert!(!logging::diagnostics_enabled());
        })
        .await;
    let context = logging::Context::new(sink.clone(), Some(operation.clone()), None, true);
    logging::CONTEXT
        .scope(context, async {
            assert!(logging::diagnostics_enabled());
            logging::emit(
                LogLevel::Info,
                EventKind::TaskStart,
                EventStage::Admission,
                None,
                None,
            );
            logging::emit(
                LogLevel::Trace,
                EventKind::HttpLogin,
                EventStage::HttpLogin,
                None,
                Some(403),
            );
            logging::helper_snapshot(Some(
                serde_json::from_value(crate::diagnostics::fixture()).unwrap(),
            ));
        })
        .await;
    sink.flush().await.unwrap();
    let page = sink
        .page(SystemLogQuery {
            operation_id: Some(operation.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.items.len(), 3);
    assert!(
        page.items
            .iter()
            .all(|e| e.operation_id.as_ref() == Some(&operation))
    );
    let text = serde_json::to_string(&page).unwrap();
    for sentinel in [
        ROOT,
        "fake-password",
        "cookie-sentinel",
        "Authorization",
        "raw_body",
    ] {
        assert!(!text.contains(sentinel));
    }
    let cookie = session(&app).await;
    for _ in 0..2000 {
        sink.emit(SystemEvent::new(LogLevel::Info, EventKind::TaskStart));
    }
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            call(
                &app,
                "GET",
                "/api/log-settings",
                Some(&cookie),
                "http://127.0.0.1:8080",
                None
            )
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
    assert!(sink.page(SystemLogQuery::default()).unwrap().dropped_count > 0);
    app.shutdown_logs().await;
}
