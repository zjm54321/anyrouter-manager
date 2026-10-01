use crate::{
    config::{Config, RootAuth},
    error::{ApiError, ErrorSource, SafeError},
    helper::LoginProvider,
    model::{Active, Candidate, Operation, id, now},
    store,
    upstream::Upstream,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use chrono::Utc;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tower_http::services::{ServeDir, ServeFile};

pub struct Accounts {
    pub active: Option<Arc<Active>>,
    pub candidate: Option<Candidate>,
    pub operation: Option<Operation>,
    sessions: HashMap<String, Instant>,
}

pub struct App {
    pub config: Config,
    root: RootAuth,
    hosts: HashSet<String>,
    pub accounts: Mutex<Accounts>,
    pub upstream: Upstream,
    pub login: Arc<dyn LoginProvider>,
}

impl App {
    pub fn new(
        mut config: Config,
        active: Option<Active>,
        upstream: Upstream,
        login: Arc<dyn LoginProvider>,
    ) -> Arc<Self> {
        let root = RootAuth::new(&config.root_key);
        config.root_key.clear();
        let hosts = config.allowed_hosts();
        Arc::new(Self {
            config,
            root,
            hosts,
            accounts: Mutex::new(Accounts {
                active: active.map(Arc::new),
                candidate: None,
                operation: None,
                sessions: HashMap::new(),
            }),
            upstream,
            login,
        })
    }

    async fn phase(&self, phase: &'static str) {
        if let Some(operation) = &mut self.accounts.lock().await.operation {
            operation.phase = phase;
        }
    }

    async fn finish(&self, result: Result<(), SafeError>) {
        if let Some(operation) = &mut self.accounts.lock().await.operation {
            operation.phase = "done";
            operation.status = if result.is_ok() {
                "succeeded"
            } else {
                "failed"
            };
            operation.error = result.err().map(|mut error| {
                if operation.kind != "login" || !self.config.login_diagnostics {
                    error.diagnostics = None;
                }
                error
            });
        }
    }
}

pub fn router(app: Arc<App>) -> Router {
    let static_files = ServeDir::new(&app.config.frontend_dir)
        .not_found_service(ServeFile::new(app.config.frontend_dir.join("index.html")));
    Router::new()
        .route(
            "/api/admin/session",
            post(create_session).delete(delete_session),
        )
        .route("/api/account", get(account))
        .route("/api/account/login", post(login))
        .route("/api/account/activate", post(activate))
        .route("/api/account/key/reveal", post(reveal))
        .route("/api/account/refresh", post(refresh))
        .route("/api", any(not_found))
        .route("/api/{*path}", any(not_found))
        .route("/v1", any(crate::gateway::proxy))
        .route("/v1/{*path}", any(crate::gateway::proxy))
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(32 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found")
}

async fn guard(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let management = request.uri().path() == "/api" || request.uri().path().starts_with("/api/");
    let host_values = request
        .headers()
        .get_all(header::HOST)
        .iter()
        .collect::<Vec<_>>();
    let host_ok = host_values.len() == 1
        && host_values[0]
            .to_str()
            .is_ok_and(|host| app.hosts.contains(host));
    let origin_values = request
        .headers()
        .get_all(header::ORIGIN)
        .iter()
        .collect::<Vec<_>>();
    let write = !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    let origin_ok = !management
        || (!write && origin_values.is_empty())
        || (origin_values.len() == 1
            && origin_values[0].to_str().is_ok_and(|origin| {
                app.config
                    .allowed_origin
                    .iter()
                    .any(|allowed| allowed == origin)
            }));
    let mut response = if !host_ok || !origin_ok {
        ApiError::new(StatusCode::FORBIDDEN, "forbidden").into_response()
    } else {
        next.run(request).await
    };
    if management {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

pub(crate) fn require_root(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let authorized = values
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|key| app.root.matches(key))
        && values.next().is_none();
    if authorized {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    let mut tokens = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|c| c.trim().strip_prefix("session_token="));
    let token = tokens.next()?.to_owned();
    if tokens.next().is_some() {
        None
    } else {
        Some(token)
    }
}

async fn require_session(app: &App, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = session_token(headers)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let mut state = app.accounts.lock().await;
    state.sessions.retain(|_, expiry| *expiry > Instant::now());
    if state.sessions.contains_key(&token) {
        Ok(token)
    } else {
        Err(ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

fn cookie_response(token: &str, max_age: u32) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let value =
        format!("session_token={token}; HttpOnly; SameSite=Strict; Path=/api/; Max-Age={max_age}");
    response.headers_mut().insert(
        header::SET_COOKIE,
        value.parse().expect("generated session cookie"),
    );
    response
}

async fn create_session(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_root(&app, &headers)?;
    if !bytes.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad_request"));
    }
    let token = format!("{}{}", id(), id());
    let mut state = app.accounts.lock().await;
    state.sessions.retain(|_, expiry| *expiry > Instant::now());
    if state.sessions.len() >= 1024 {
        return Err(ApiError::new(StatusCode::CONFLICT, "operation_in_progress"));
    }
    state.sessions.insert(
        token.clone(),
        Instant::now() + Duration::from_secs(8 * 3600),
    );
    Ok(cookie_response(&token, 28800))
}

async fn delete_session(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let token = require_session(&app, &headers).await?;
    if !bytes.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad_request"));
    }
    app.accounts.lock().await.sessions.remove(&token);
    Ok(cookie_response("", 0))
}

async fn account(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_session(&app, &headers).await?;
    let mut state = app.accounts.lock().await;
    if state
        .candidate
        .as_ref()
        .is_some_and(|candidate| candidate.deadline <= Instant::now())
    {
        state.candidate = None;
    }
    Ok(Json(
        serde_json::json!({"active": state.active.as_ref().map(|a| a.dto()),
        "candidate": state.candidate.as_ref().map(Candidate::dto), "operation": state.operation}),
    ))
}

fn body<T: serde::de::DeserializeOwned>(
    input: Result<Json<T>, axum::extract::rejection::JsonRejection>,
) -> Result<T, ApiError> {
    input
        .map(|Json(value)| value)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "bad_request"))
}

fn idle(state: &Accounts) -> Result<(), ApiError> {
    if state
        .operation
        .as_ref()
        .is_some_and(|o| o.status == "running")
    {
        Err(ApiError::new(StatusCode::CONFLICT, "operation_in_progress"))
    } else {
        Ok(())
    }
}

fn start(state: &mut Accounts, kind: &'static str, phase: &'static str) -> Response {
    let operation_id = id();
    state.operation = Some(Operation {
        id: operation_id.clone(),
        kind,
        status: "running",
        phase,
        error: None,
    });
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"operation_id": operation_id})),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginInput {
    username: String,
    password: String,
}

async fn login(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    input: Result<Json<LoginInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    require_session(&app, &headers).await?;
    let input = body(input)?;
    if input.username.trim().is_empty()
        || input.username.len() > 320
        || input.password.is_empty()
        || input.password.len() > 4096
        || input
            .username
            .chars()
            .chain(input.password.chars())
            .any(|c| c.is_control())
    {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad_request"));
    }
    let response = {
        let mut state = app.accounts.lock().await;
        idle(&state)?;
        start(&mut state, "login", "authenticating")
    };
    tokio::spawn(async move {
        let result = prepare_candidate(&app, input).await;
        app.finish(result).await;
    });
    Ok(response)
}

async fn prepare_candidate(app: &App, input: LoginInput) -> Result<(), SafeError> {
    let credentials = app
        .login
        .login(input.username.clone(), input.password)
        .await
        .map_err(|mut error| {
            if error.source.is_none() {
                error.source = Some(ErrorSource::Helper);
            }
            error
        })?;
    if !credentials.validate() {
        return Err(
            SafeError::new("upstream_unexpected_response").with_source(ErrorSource::Credentials)
        );
    }
    app.phase("reading_account").await;
    let balance = app
        .upstream
        .balance(&credentials)
        .await
        .map_err(|e| e.with_source(ErrorSource::SelfAccount))?;
    app.phase("listing_keys").await;
    let keys = app
        .upstream
        .keys(&credentials)
        .await
        .map_err(|e| e.with_source(ErrorSource::Tokens))?;
    let candidate = Candidate {
        id: id(),
        username: input.username,
        credentials,
        balance,
        keys,
        deadline: Instant::now() + Duration::from_secs(900),
        expires_at: (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339(),
    };
    app.accounts.lock().await.candidate = Some(candidate);
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateInput {
    candidate_id: String,
    key_id: String,
}

async fn activate(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    input: Result<Json<ActivateInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    require_session(&app, &headers).await?;
    let input = body(input)?;
    let (candidate, key, response) = {
        let mut state = app.accounts.lock().await;
        idle(&state)?;
        let candidate = state
            .candidate
            .as_ref()
            .filter(|c| c.deadline > Instant::now())
            .ok_or_else(|| ApiError::new(StatusCode::GONE, "candidate_expired"))?
            .clone();
        if candidate.id != input.candidate_id {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad_request"));
        }
        let key = candidate
            .keys
            .iter()
            .find(|k| k.id == input.key_id && k.enabled)
            .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "bad_request"))?
            .clone();
        let response = start(&mut state, "activate", "reading_key");
        (candidate, key, response)
    };
    tokio::spawn(async move {
        let result = async {
            let raw = app
                .upstream
                .key(&candidate.credentials, &key)
                .await
                .map_err(|e| e.with_source(ErrorSource::Key))?;
            if !candidate.credentials.validate() {
                return Err(SafeError::new("upstream_session_expired")
                    .with_source(ErrorSource::Credentials));
            }
            app.phase("committing").await;
            let mut state = app.accounts.lock().await;
            let revision = next_revision(state.active.as_deref())?;
            let active = Active {
                revision,
                username: candidate.username,
                upstream_user_id: candidate.credentials.api_user.clone(),
                balance: candidate.balance,
                key_id: key.id,
                key_name: key.name,
                key: raw,
                credentials: candidate.credentials,
                activated_at: now(),
            };
            store::save(&app.config.state_path, &active)
                .map_err(|e| e.with_source(ErrorSource::Persistence))?;
            state.active = Some(Arc::new(active));
            state.candidate = None;
            Ok(())
        }
        .await;
        app.finish(result).await;
    });
    Ok(response)
}

fn next_revision(active: Option<&Active>) -> Result<String, SafeError> {
    let revision = active
        .map_or(Some(0), |a| a.revision.parse::<u64>().ok())
        .and_then(|r| r.checked_add(1))
        .ok_or_else(|| {
            SafeError::new("persistence_failed").with_source(ErrorSource::Persistence)
        })?;
    Ok(revision.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevealInput {
    active_revision: String,
}

async fn reveal(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    input: Result<Json<RevealInput>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_session(&app, &headers).await?;
    require_root(&app, &headers)?;
    let input = body(input)?;
    let state = app.accounts.lock().await;
    let active = state
        .active
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "account_not_configured"))?;
    if input.active_revision != active.revision {
        return Err(ApiError::new(StatusCode::CONFLICT, "stale_revision"));
    }
    Ok(Json(
        serde_json::json!({"active_revision": active.revision, "key_id": active.key_id, "key": active.key}),
    ))
}

async fn refresh(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<Response, ApiError> {
    require_session(&app, &headers).await?;
    if !bytes.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "bad_request"));
    }
    let (active, response) = {
        let mut state = app.accounts.lock().await;
        idle(&state)?;
        let active = state
            .active
            .clone()
            .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "account_not_configured"))?;
        let response = start(&mut state, "refresh", "reading_account");
        (active, response)
    };
    tokio::spawn(async move {
        let result = async {
            let balance = app
                .upstream
                .balance(&active.credentials)
                .await
                .map_err(|e| e.with_source(ErrorSource::SelfAccount))?;
            app.phase("committing").await;
            let mut updated = (*active).clone();
            updated.balance = balance;
            updated.revision = next_revision(Some(&active))?;
            let mut state = app.accounts.lock().await;
            store::save(&app.config.state_path, &updated)
                .map_err(|e| e.with_source(ErrorSource::Persistence))?;
            state.active = Some(Arc::new(updated));
            Ok(())
        }
        .await;
        app.finish(result).await;
    });
    Ok(response)
}
