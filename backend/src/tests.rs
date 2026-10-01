use crate::{
    app::{self, App},
    config::Config,
    error::{ErrorSource, SafeError},
    helper::LoginProvider,
    model::{self, Active, Balance, Cookie, Credentials},
    store,
    upstream::Upstream,
};
use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tower::ServiceExt;

const ROOT: &str = "Test-Only-Root-91c7e8aa-2345-SufficientEntropy";
const KEY: &str = "exact-token-no-invented-prefix";
const SECRET_COOKIE: &str = "private-upstream-cookie";

struct FakeLogin {
    fail: AtomicBool,
}
#[async_trait]
impl LoginProvider for FakeLogin {
    async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
        if self.fail.load(Ordering::Relaxed) {
            Err(SafeError::new("invalid_credentials"))
        } else {
            Ok(credentials())
        }
    }
}

fn credentials() -> Credentials {
    Credentials {
        api_user: "7".into(),
        cookies: vec![Cookie {
            name: "session".into(),
            value: SECRET_COOKIE.into(),
            domain: "anyrouter.top".into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires: None,
        }],
    }
}

fn active() -> Active {
    Active {
        revision: "1".into(),
        username: "old-account".into(),
        upstream_user_id: "7".into(),
        balance: Balance {
            quota_raw: "12345678901234567890".into(),
            used_quota_raw: "2".into(),
            fetched_at: model::now(),
        },
        key_id: "1".into(),
        key_name: "existing".into(),
        key: KEY.into(),
        credentials: credentials(),
        activated_at: model::now(),
    }
}

#[derive(Default)]
struct MockState {
    refresh_fail: AtomicBool,
    tokens_fail: AtomicBool,
}

async fn mock(State(state): State<Arc<MockState>>, request: Request) -> Response {
    let path = request.uri().path();
    if path == "/api/user/self" {
        assert_eq!(request.headers()["new-api-user"], "7");
        assert!(
            request.headers()[header::COOKIE]
                .to_str()
                .unwrap()
                .contains(SECRET_COOKIE)
        );
        if state.refresh_fail.load(Ordering::Relaxed) {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"success":false})),
            )
                .into_response();
        }
        return axum::Json(
            json!({"success":true,"data":{"id":7,"quota":"99999999999999999999","used_quota":3}}),
        )
        .into_response();
    }
    if path == "/api/token/" {
        if state.tokens_fail.load(Ordering::Relaxed) {
            return axum::Json(
                json!({"success": false, "message": "sentinel-upstream-body-token-password"}),
            )
            .into_response();
        }
        let page = request.uri().query().unwrap();
        let items = if page.starts_with("p=0&") {
            (1..=100)
                .map(|id| json!({"id":id,"name":format!("key-{id}"),"status":1,"key":"********"}))
                .collect::<Vec<_>>()
        } else {
            vec![json!({"id":101,"name":"last-key","status":1,"key":KEY})]
        };
        return axum::Json(json!({"success":true,"data":{"items":items,"total":101}}))
            .into_response();
    }
    if path == "/api/token/1/key" {
        assert_eq!(request.method(), "POST");
        return axum::Json(json!({"success":true,"data":{"key":KEY}})).into_response();
    }
    if path == "/v1/redirect" {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [
                ("location", "/v1/must-not-follow"),
                ("set-cookie", "upstream=secret"),
            ],
            "redirect-body",
        )
            .into_response();
    }
    if path == "/v1/events" {
        let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(b"data: one\n\n")),
            Ok(Bytes::from_static(b"data: two\n\n")),
        ];
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(futures_util::stream::iter(chunks)),
        )
            .into_response();
    }
    if path == "/v1/echo" {
        assert_eq!(request.method(), "PATCH");
        assert_eq!(request.uri().query(), Some("a=%2F&a=+&b=%25&empty="));
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            format!("Bearer {KEY}")
        );
        assert!(!request.headers().contains_key(header::COOKIE));
        assert!(!request.headers().contains_key("x-hop-secret"));
        assert!(!request.headers().contains_key("x-admin-secret"));
        let body = to_bytes(request.into_body(), 1024).await.unwrap();
        return (
            StatusCode::IM_A_TEAPOT,
            [
                ("connection", "x-private"),
                ("x-private", "drop-me"),
                ("set-cookie", "upstream=secret"),
            ],
            body,
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "mock unknown").into_response()
}

struct Fixture {
    app: Arc<App>,
    router: Router,
    fake: Arc<FakeLogin>,
    mock: Arc<MockState>,
    task: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn new(with_active: bool) -> Self {
        Self::with_login(with_active, false, None).await
    }

    async fn with_login(
        with_active: bool,
        diagnostics: bool,
        login: Option<Arc<dyn LoginProvider>>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mock_state = Arc::new(MockState::default());
        let upstream_router = Router::new()
            .fallback(any(mock))
            .with_state(mock_state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, upstream_router).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            root_key: ROOT.into(),
            state_path: directory.path().join("data/account.json"),
            login_diagnostics: diagnostics,
            ..Config::default()
        };
        config.validate().unwrap();
        let fake = Arc::new(FakeLogin {
            fail: AtomicBool::new(false),
        });
        let app = App::new(
            config,
            with_active.then(active),
            Upstream::mock(base),
            login.unwrap_or_else(|| fake.clone()),
        );
        Self {
            router: app::router(app.clone()),
            app,
            fake,
            mock: mock_state,
            task,
            _directory: directory,
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        root: bool,
        input: Option<Value>,
    ) -> Response {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:8080")
            .header("origin", "http://localhost:5173");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        if root {
            request = request.header("authorization", format!("Bearer {ROOT}"));
        }
        let body = match input {
            Some(input) => {
                request = request.header("content-type", "application/json");
                Body::from(input.to_string())
            }
            None => Body::empty(),
        };
        self.router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn session(&self) -> String {
        let response = self
            .request("POST", "/api/admin/session", None, true, None)
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Strict; Path=/api/; Max-Age=28800"));
        cookie.split(';').next().unwrap().to_owned()
    }

    async fn done(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self
                    .app
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
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

struct DiagnosticFailure;
#[async_trait]
impl LoginProvider for DiagnosticFailure {
    async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
        let mut error = SafeError::new("upstream_login_failed").with_source(ErrorSource::Helper);
        error.diagnostics = Some(serde_json::from_value(crate::diagnostics::fixture()).unwrap());
        Err(error)
    }
}

#[tokio::test]
async fn management_diagnostics_opt_in_only_and_auth_shape_unchanged() {
    for enabled in [false, true] {
        let f = Fixture::with_login(true, enabled, Some(Arc::new(DiagnosticFailure))).await;
        let unauthorized = f.request("GET", "/api/account", None, false, None).await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let auth = json_body(unauthorized).await;
        assert_eq!(auth["error"].as_object().unwrap().len(), 2);
        let cookie = f.session().await;
        let response = f
            .request(
                "POST",
                "/api/account/login",
                Some(&cookie),
                false,
                Some(json!({"username": "fake", "password": "sentinel-password"})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        f.done().await;
        let response = f
            .request("GET", "/api/account", Some(&cookie), false, None)
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let dto = json_body(response).await;
        assert_eq!(dto["operation"]["phase"], "done");
        assert_eq!(dto["operation"]["error"]["source"], "helper");
        assert_eq!(
            dto["operation"]["error"].get("diagnostics"),
            enabled.then_some(&crate::diagnostics::fixture())
        );
        assert!(
            !dto["active"]
                .as_object()
                .unwrap()
                .contains_key("diagnostics")
        );
        let text = dto.to_string();
        for secret in ["sentinel-password", SECRET_COOKIE, KEY, ROOT] {
            assert!(!text.contains(secret));
        }
        assert_eq!(
            f.app
                .accounts
                .lock()
                .await
                .active
                .as_ref()
                .unwrap()
                .revision,
            "1"
        );
    }
}

#[tokio::test]
async fn login_stage_sources_self_and_tokens_do_not_reflect_upstream() {
    for tokens in [false, true] {
        let f = Fixture::new(true).await;
        if tokens {
            f.mock.tokens_fail.store(true, Ordering::Relaxed);
        } else {
            f.mock.refresh_fail.store(true, Ordering::Relaxed);
        }
        let cookie = f.session().await;
        let response = f
            .request(
                "POST",
                "/api/account/login",
                Some(&cookie),
                false,
                Some(json!({"username": "fake", "password": "sentinel-password"})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        f.done().await;
        let response = f
            .request("GET", "/api/account", Some(&cookie), false, None)
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let dto = json_body(response).await;
        assert_eq!(dto["operation"]["status"], "failed");
        assert_eq!(dto["operation"]["phase"], "done");
        assert_eq!(
            dto["operation"]["error"]["source"],
            if tokens { "tokens" } else { "self" }
        );
        assert!(dto["operation"]["error"].get("diagnostics").is_none());
        let text = dto.to_string();
        for secret in ["sentinel", KEY, SECRET_COOKIE, ROOT] {
            assert!(!text.contains(secret));
        }
        assert_eq!(dto["active"]["revision"], "1");
    }
}

#[tokio::test]
async fn session_auth_origin_host_no_store_and_unknown_routes() {
    let f = Fixture::new(false).await;
    let response = f
        .request("POST", "/api/admin/session", None, false, None)
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let wrong_root = axum::http::Request::builder()
        .method("POST")
        .uri("/api/admin/session")
        .header("host", "127.0.0.1:8080")
        .header("origin", "http://localhost:5173")
        .header("authorization", "Bearer definitely-wrong-root")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.router.clone().oneshot(wrong_root).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/admin/session")
        .header("host", "evil.invalid:8080")
        .header("origin", "http://evil.invalid")
        .header("authorization", format!("Bearer {ROOT}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.router.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/admin/session")
        .header("host", "127.0.0.1:8080")
        .header("authorization", format!("Bearer {ROOT}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.router.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let cookie = f.session().await;
    assert_eq!(
        f.request("GET", "/api/account", None, true, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request("GET", "/api/unknown", Some(&cookie), false, None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("DELETE", "/api/admin/session", Some(&cookie), false, None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.request("GET", "/api/account", Some(&cookie), false, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn login_candidate_activation_reveal_refresh_and_failure_retention() {
    let f = Fixture::new(true).await;
    let cookie = f.session().await;
    assert_eq!(
        f.request(
            "POST",
            "/api/account/login",
            Some(&cookie),
            false,
            Some(json!({"username":"new-account","password":"never-persist-this-password"}))
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    f.done().await;
    let dto = json_body(
        f.request("GET", "/api/account", Some(&cookie), false, None)
            .await,
    )
    .await;
    assert_eq!(dto["active"]["username"], "old-account");
    assert_eq!(dto["candidate"]["keys"].as_array().unwrap().len(), 101);
    let text = dto.to_string();
    for secret in [KEY, SECRET_COOKIE, "never-persist-this-password"] {
        assert!(!text.contains(secret));
    }
    let candidate_id = dto["candidate"]["id"].as_str().unwrap().to_owned();
    f.fake.fail.store(true, Ordering::Relaxed);
    f.request(
        "POST",
        "/api/account/login",
        Some(&cookie),
        false,
        Some(json!({"username":"bad","password":"bad"})),
    )
    .await;
    f.done().await;
    let dto = json_body(
        f.request("GET", "/api/account", Some(&cookie), false, None)
            .await,
    )
    .await;
    assert_eq!(dto["candidate"]["id"], candidate_id);
    assert_eq!(dto["active"]["username"], "old-account");
    assert_eq!(dto["operation"]["error"]["code"], "invalid_credentials");
    assert_eq!(
        f.request(
            "POST",
            "/api/account/activate",
            Some(&cookie),
            false,
            Some(json!({"candidate_id":candidate_id,"key_id":"1"}))
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    f.done().await;
    let dto = json_body(
        f.request("GET", "/api/account", Some(&cookie), false, None)
            .await,
    )
    .await;
    assert_eq!(dto["active"]["revision"], "2");
    assert!(dto["candidate"].is_null());
    assert_eq!(
        f.request(
            "POST",
            "/api/account/key/reveal",
            Some(&cookie),
            false,
            Some(json!({"active_revision":"2"}))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let stale = f
        .request(
            "POST",
            "/api/account/key/reveal",
            Some(&cookie),
            true,
            Some(json!({"active_revision":"1"})),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "stale_revision");
    assert_eq!(
        json_body(
            f.request(
                "POST",
                "/api/account/key/reveal",
                Some(&cookie),
                true,
                Some(json!({"active_revision":"2"}))
            )
            .await
        )
        .await["key"],
        KEY
    );
    let bytes = std::fs::read(&f.app.config.state_path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("password"));
    assert_eq!(
        std::fs::metadata(&f.app.config.state_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(f.app.config.state_path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(store::load(&f.app.config.state_path).unwrap().is_some());
    f.request("POST", "/api/account/refresh", Some(&cookie), false, None)
        .await;
    f.done().await;
    assert_eq!(
        f.app
            .accounts
            .lock()
            .await
            .active
            .as_ref()
            .unwrap()
            .revision,
        "3"
    );
    f.mock.refresh_fail.store(true, Ordering::Relaxed);
    f.request("POST", "/api/account/refresh", Some(&cookie), false, None)
        .await;
    f.done().await;
    let state = f.app.accounts.lock().await;
    assert_eq!(state.active.as_ref().unwrap().revision, "3");
    assert_eq!(state.active.as_ref().unwrap().key, KEY);
    assert_eq!(
        state
            .operation
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .code,
        "upstream_session_expired"
    );
}

#[tokio::test]
async fn expired_candidate_single_operation_and_persistence_failure() {
    let f = Fixture::new(true).await;
    let cookie = f.session().await;
    f.request(
        "POST",
        "/api/account/login",
        Some(&cookie),
        false,
        Some(json!({"username":"new","password":"test"})),
    )
    .await;
    f.done().await;
    let candidate_id = f
        .app
        .accounts
        .lock()
        .await
        .candidate
        .as_ref()
        .unwrap()
        .id
        .clone();
    let old_candidate = f.app.accounts.lock().await.candidate.clone();
    f.app
        .accounts
        .lock()
        .await
        .candidate
        .as_mut()
        .unwrap()
        .deadline = Instant::now() - Duration::from_secs(1);
    assert_eq!(
        f.request(
            "POST",
            "/api/account/activate",
            Some(&cookie),
            false,
            Some(json!({"candidate_id":candidate_id,"key_id":"1"}))
        )
        .await
        .status(),
        StatusCode::GONE
    );
    f.app.accounts.lock().await.candidate = old_candidate;
    std::fs::create_dir_all(f.app.config.state_path.parent().unwrap()).unwrap();
    std::fs::create_dir(&f.app.config.state_path).unwrap();
    f.request(
        "POST",
        "/api/account/activate",
        Some(&cookie),
        false,
        Some(json!({"candidate_id":candidate_id,"key_id":"1"})),
    )
    .await;
    f.done().await;
    let mut state = f.app.accounts.lock().await;
    assert_eq!(state.active.as_ref().unwrap().username, "old-account");
    assert!(state.candidate.is_some());
    assert_eq!(
        state
            .operation
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .code,
        "persistence_failed"
    );
    state.operation.as_mut().unwrap().status = "running";
    drop(state);
    assert_eq!(
        f.request("POST", "/api/account/refresh", Some(&cookie), false, None)
            .await
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn gateway_bytes_query_headers_status_sse_redirect_and_missing_key() {
    let f = Fixture::new(true).await;
    let request = axum::http::Request::builder()
        .method("PATCH")
        .uri("/v1/echo?a=%2F&a=+&b=%25&empty=")
        .header("host", "127.0.0.1:8080")
        .header("authorization", format!("Bearer {ROOT}"))
        .header("cookie", "session_token=local-secret")
        .header("connection", "x-hop-secret")
        .header("x-hop-secret", "sensitive")
        .header("x-admin-secret", "sensitive")
        .body(Body::from(vec![0, 255, 12, 13, 10, 128]))
        .unwrap();
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert!(!response.headers().contains_key("set-cookie"));
    assert!(!response.headers().contains_key("x-private"));
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        &[0, 255, 12, 13, 10, 128]
    );
    let response = f.request("GET", "/v1/redirect", None, true, None).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()["location"], "/v1/must-not-follow");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "redirect-body"
    );
    let response = f.request("GET", "/v1/events", None, true, None).await;
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "data: one\n\ndata: two\n\n"
    );
    assert_eq!(
        f.request("GET", "/v1", None, false, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    f.app.accounts.lock().await.active = None;
    assert_eq!(
        f.request("GET", "/v1", None, true, None).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn upstream_challenge_unexpected_shapes_and_key_bounds() {
    async fn response(State(mode): State<&'static str>, request: Request) -> Response {
        match mode {
            "challenge" => (
                StatusCode::FORBIDDEN,
                "<html><script src='challenge-platform/cf-chl-x'></script></html>",
            )
                .into_response(),
            "html" => (StatusCode::OK, "<html>private-secret-body</html>").into_response(),
            "wrong-id" => {
                axum::Json(json!({"success":true,"data":{"id":8,"quota":1,"used_quota":0}}))
                    .into_response()
            }
            "empty" => axum::Json(json!({"success":true,"data":[]})).into_response(),
            "bad-shape" => axum::Json(json!({"success":true,"data":{}})).into_response(),
            "too-many" => {
                axum::Json(json!({"success":true,"data":{"items":[],"total":2001}})).into_response()
            }
            "full-pages" => {
                let page = request
                    .uri()
                    .query()
                    .unwrap()
                    .split('&')
                    .next()
                    .unwrap()
                    .strip_prefix("p=")
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                let items = (1..=100)
                    .map(|id| json!({"id":page*100+id,"name":"key","status":1,"key":"masked"}))
                    .collect::<Vec<_>>();
                axum::Json(json!({"success":true,"data":items})).into_response()
            }
            _ => unreachable!(),
        }
    }
    for (mode, expected) in [
        ("challenge", "upstream_challenge"),
        ("html", "upstream_unexpected_response"),
        ("wrong-id", "upstream_unexpected_response"),
        ("empty", "ok"),
        ("bad-shape", "upstream_unexpected_response"),
        ("too-many", "too_many_keys"),
        ("full-pages", "too_many_keys"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = Upstream::mock(format!("http://{}", listener.local_addr().unwrap()));
        let router = Router::new().fallback(any(response)).with_state(mode);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        if ["challenge", "html", "wrong-id"].contains(&mode) {
            let error = transport.balance(&credentials()).await.err().unwrap();
            assert_eq!(error.code, expected);
            assert!(!error.message.contains("private-secret-body"));
        } else {
            let result = transport.keys(&credentials()).await;
            if expected == "ok" {
                assert!(result.unwrap().is_empty());
            } else {
                assert_eq!(result.err().unwrap().code, expected);
            }
        }
        task.abort();
    }
}

#[tokio::test]
async fn gateway_delivers_sse_before_upstream_finishes() {
    async fn delayed() -> Response {
        let stream = futures_util::stream::unfold(0, |index| async move {
            match index {
                0 => Some((
                    Ok::<_, std::io::Error>(Bytes::from_static(b"data: first\n\n")),
                    1,
                )),
                1 => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Some((Ok(Bytes::from_static(b"data: last\n\n")), 2))
                }
                _ => None,
            }
        });
        (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let transport = Upstream::mock(format!("http://{}", listener.local_addr().unwrap()));
    let task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(any(delayed)))
            .await
            .unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        root_key: ROOT.into(),
        state_path: directory.path().join("state/account.json"),
        ..Config::default()
    };
    let app = App::new(
        config,
        Some(active()),
        transport,
        Arc::new(FakeLogin {
            fail: AtomicBool::new(false),
        }),
    );
    let request = axum::http::Request::builder()
        .uri("/v1/events")
        .header("host", "localhost:8080")
        .header("authorization", format!("Bearer {ROOT}"))
        .body(Body::empty())
        .unwrap();
    let response = app::router(app).oneshot(request).await.unwrap();
    use futures_util::StreamExt;
    let mut stream = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(Duration::from_millis(500), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(chunk, "data: first\n\n");
    drop(stream);
    task.abort();
}

#[tokio::test]
async fn expired_persisted_session_keeps_gateway_and_refresh_retains_snapshot() {
    let f = Fixture::new(true).await;
    let mut old = active();
    old.credentials.cookies[0].expires = Some(1.0);
    assert!(!old.credentials.validate());
    assert!(old.validate());
    store::save(&f.app.config.state_path, &old).unwrap();
    let loaded = store::load(&f.app.config.state_path).unwrap().unwrap();
    f.app.accounts.lock().await.active = Some(Arc::new(loaded));
    let cookie = f.session().await;
    let response = f.request("GET", "/v1/events", None, true, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = to_bytes(response.into_body(), 1024).await.unwrap();
    let before = std::fs::read(&f.app.config.state_path).unwrap();
    f.request("POST", "/api/account/refresh", Some(&cookie), false, None)
        .await;
    f.done().await;
    let state = f.app.accounts.lock().await;
    assert_eq!(
        state
            .operation
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .code,
        "upstream_session_expired"
    );
    assert_eq!(state.active.as_ref().unwrap().key, KEY);
    assert_eq!(state.active.as_ref().unwrap().revision, "1");
    assert_eq!(std::fs::read(&f.app.config.state_path).unwrap(), before);
    drop(state);
    // A fresh login can still be started after the failed refresh.
    assert_eq!(
        f.request(
            "POST",
            "/api/account/login",
            Some(&cookie),
            false,
            Some(json!({"username":"new","password":"fake-test-password"}))
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    f.done().await;
}

#[test]
fn cookie_structure_requires_finite_expiry_but_not_live_session() {
    let mut credentials = credentials();
    for expiry in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -2.0] {
        credentials.cookies[0].expires = Some(expiry);
        assert!(!credentials.validate_structure());
    }
    credentials.cookies[0].expires = Some(1.0);
    assert!(credentials.validate_structure());
    assert!(!credentials.validate());
}

#[test]
fn failed_actual_rename_preserves_existing_state_bytes_and_checksum() {
    use sha2::{Digest, Sha256};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("data/account.json");
    let old = active();
    store::save(&path, &old).unwrap();
    let before = std::fs::read(&path).unwrap();
    let checksum = Sha256::digest(&before);
    let mut updated = old.clone();
    updated.key = "new-fake-test-key".into();
    updated.revision = "2".into();
    assert_eq!(
        store::save_with_rename_failure(&path, &updated)
            .unwrap_err()
            .code,
        "persistence_failed"
    );
    let after = std::fs::read(&path).unwrap();
    assert_eq!(after, before);
    assert_eq!(Sha256::digest(&after), checksum);
    let reloaded = store::load(&path).unwrap().unwrap();
    assert_eq!(reloaded.key, KEY);
    assert_eq!(reloaded.revision, "1");
}

// These probes run two real HTTP servers and a TCP downstream client. The signal
// comes from upstream's actual response-stream drop or request-body transport
// error, not from dropping a router oneshot future or a synthetic counter.
struct DisconnectProbe {
    started: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    disconnected: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

struct StreamDisconnect(Option<tokio::sync::oneshot::Sender<()>>);
impl Drop for StreamDisconnect {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

async fn disconnect_upstream(
    State(probe): State<Arc<DisconnectProbe>>,
    request: Request,
) -> Response {
    if request.uri().path() == "/v1/upload" {
        use futures_util::StreamExt;
        let mut stream = request.into_body().into_data_stream();
        assert!(stream.next().await.unwrap().is_ok());
        if let Some(sender) = probe.started.lock().await.take() {
            let _ = sender.send(());
        }
        while let Some(chunk) = stream.next().await {
            if chunk.is_err() {
                if let Some(sender) = probe.disconnected.lock().await.take() {
                    let _ = sender.send(());
                }
                break;
            }
        }
        return StatusCode::OK.into_response();
    }
    let guard = StreamDisconnect(probe.disconnected.lock().await.take());
    if let Some(sender) = probe.started.lock().await.take() {
        let _ = sender.send(());
    }
    let stream = futures_util::stream::unfold((guard, 0), |(guard, count)| async move {
        if count != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Some((
            Ok::<_, std::io::Error>(Bytes::from_static(b"data: alive\n\n")),
            (guard, count + 1),
        ))
    });
    (
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn network_disconnect_test(upload: bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (disconnected_tx, disconnected_rx) = tokio::sync::oneshot::channel();
    let probe = Arc::new(DisconnectProbe {
        started: tokio::sync::Mutex::new(Some(started_tx)),
        disconnected: tokio::sync::Mutex::new(Some(disconnected_tx)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let transport = Upstream::mock(format!("http://{}", listener.local_addr().unwrap()));
    let upstream_task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .fallback(any(disconnect_upstream))
                .with_state(probe),
        )
        .await
        .unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        root_key: ROOT.into(),
        state_path: directory.path().join("state/account.json"),
        ..Config::default()
    };
    let app = App::new(
        config,
        Some(active()),
        transport,
        Arc::new(FakeLogin {
            fail: AtomicBool::new(false),
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let downstream_task = tokio::spawn(async move {
        axum::serve(listener, app::router(app)).await.unwrap();
    });
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = if upload {
        format!(
            "POST /v1/upload HTTP/1.1\r\nHost: localhost:8080\r\nAuthorization: Bearer {ROOT}\r\nContent-Length: 1000000\r\n\r\nfirst-incomplete-upload"
        )
    } else {
        format!(
            "GET /v1/events HTTP/1.1\r\nHost: localhost:8080\r\nAuthorization: Bearer {ROOT}\r\n\r\n"
        )
    };
    socket.write_all(request.as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), started_rx)
        .await
        .unwrap()
        .unwrap();
    if !upload {
        let mut bytes = [0; 1024];
        let count = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(count > 0);
    }
    // Actual downstream TCP close, without ever completing the upload body.
    drop(socket);
    let observed = tokio::time::timeout(Duration::from_secs(3), disconnected_rx).await;
    downstream_task.abort();
    upstream_task.abort();
    observed
        .expect("upstream must observe downstream disconnect within bound")
        .unwrap();
}

#[tokio::test]
async fn downstream_tcp_sse_abort_drops_upstream_response_stream() {
    network_disconnect_test(false).await;
}

#[tokio::test]
async fn downstream_tcp_upload_abort_cancels_upstream_request_body() {
    network_disconnect_test(true).await;
}

#[test]
fn config_material_and_target_validation() {
    let mut config = Config::default();
    assert!(config.validate().is_err());
    config.root_key = "REPLACE_WITH_A_RANDOM_ROOT_KEY_AT_LEAST_32_BYTES".into();
    assert!(config.validate().is_err());
    config.root_key = ROOT.into();
    assert!(config.validate().is_ok());
    assert!(
        toml::from_str::<Config>(&format!(
            "root_key = '{ROOT}'\nupstream_url = 'http://evil' "
        ))
        .is_err()
    );
    for key in [
        "********",
        "sk-...",
        "••••",
        "masked",
        "[REDACTED]",
        "bad\r\nheader",
    ] {
        assert!(!model::full_key(key));
    }
    assert!(model::full_key(KEY));
    for path in [
        "http://evil/v1",
        "/v1/../admin",
        "/v1/%2e%2e/admin",
        "/v1/%2f%2fevil",
        "/v1/back%5cslash",
    ] {
        assert!(
            crate::gateway::target("https://anyrouter.top", &path.parse().unwrap()).is_err(),
            "{path}"
        );
    }
    assert_eq!(
        crate::gateway::target(
            "https://anyrouter.top",
            &"/v1/a?q=%2F+%25&x=".parse().unwrap()
        )
        .unwrap()
        .as_str(),
        "https://anyrouter.top/v1/a?q=%2F+%25&x="
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("data/state.json");
    store::save(&path, &active()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store::load(&path).is_err());
}

#[test]
fn cookie_selection_obeys_domain_secure_expiry_path_and_precedence() {
    let mut credentials = credentials();
    let original = credentials.cookies[0].clone();
    for (path, value) in [
        ("/api", "api"),
        ("/api/user", "user"),
        ("/ap", "wrong-boundary"),
        ("/console", "console"),
        ("/api/token", "tokens"),
    ] {
        credentials.cookies.push(Cookie {
            path: path.into(),
            value: value.into(),
            ..original.clone()
        });
    }
    credentials.cookies.push(Cookie {
        domain: "evil.invalid".into(),
        value: "wrong-domain".into(),
        ..original.clone()
    });
    credentials.cookies.push(Cookie {
        value: "expired".into(),
        expires: Some(1.0),
        ..original
    });
    let selected = credentials
        .cookie_header_for("https://anyrouter.top/api/user/self")
        .unwrap();
    assert_eq!(
        selected,
        format!("session=user; session=api; session={SECRET_COOKIE}")
    );
    assert!(
        credentials
            .cookie_header_for("http://anyrouter.top/api/user/self")
            .is_none()
    );
    assert_eq!(
        credentials
            .cookie_header_for("https://anyrouter.top/api/token/?p=0")
            .unwrap(),
        format!("session=tokens; session=api; session={SECRET_COOKIE}")
    );
    credentials
        .cookies
        .retain(|cookie| matches!(cookie.domain.as_str(), "anyrouter.top" | ".anyrouter.top"));
    assert!(credentials.validate());
}
