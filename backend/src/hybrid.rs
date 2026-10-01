//! HTTP-first authentication. Every attempt has a fresh standards-based jar;
//! neither persisted sessions nor the streaming gateway client are reused.
use crate::{
    error::{ErrorSource, SafeError},
    helper::{LoginOutcome, LoginProvider},
    model::{Cookie, Credentials},
    upstream::{UPSTREAM, USER_AGENT, identifier, profile_data},
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest_cookie_store::{CookieStore, CookieStoreMutex};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

pub struct HybridLoginProvider {
    browser: Arc<dyn LoginProvider>,
    timeout: Duration,
    base: String,
}

impl HybridLoginProvider {
    pub fn new(browser: Arc<dyn LoginProvider>, timeout: Duration) -> Self {
        Self {
            browser,
            timeout,
            base: UPSTREAM.into(),
        }
    }

    async fn attempt(&self, username: String, password: String) -> Result<LoginOutcome, SafeError> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        let jar = Arc::new(CookieStoreMutex::new(CookieStore::default()));
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .user_agent(USER_AGENT)
            .cookie_provider(jar.clone())
            .build()
            .map_err(|_| unavailable())?;
        let response = client
            .post(format!("{}/api/user/login", self.base))
            .json(&serde_json::json!({"username":username,"password":password}))
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let bytes = bounded(response).await?;
        // JSON refusals, network errors, redirects and 5xx never trigger Chrome.
        if status.is_redirection() || status.is_server_error() {
            return Err(unavailable());
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) if recognized_waf(&bytes) => {
                let budget = deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .saturating_sub(Duration::from_millis(250));
                let credentials = self
                    .browser
                    .login_with_budget(username, password, budget)
                    .await?;
                return Ok(LoginOutcome {
                    credentials,
                    profile: None,
                });
            }
            Err(_) => return Err(unexpected()),
        };
        if value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(
                SafeError::new("upstream_login_failed").with_source(ErrorSource::Credentials)
            );
        }
        if !status.is_success() || value.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(unexpected());
        }
        let user = identifier(&value["data"]["id"])?;
        let credentials = extract(&jar, &user, &self.base)?;
        if !credentials.validate() {
            return Err(unverified());
        }
        let response = client
            .get(format!("{}/api/user/self", self.base))
            .header("New-Api-User", &user)
            .send()
            .await
            .map_err(transport)?;
        if !response.status().is_success() {
            return Err(unverified());
        }
        let value: Value =
            serde_json::from_slice(&bounded(response).await?).map_err(|_| unverified())?;
        if value.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(unverified());
        }
        let profile = profile_data(&value["data"], &user).map_err(|_| unverified())?;
        // Include validated cookie updates/deletions from self, not a stale login snapshot.
        let credentials = extract(&jar, &user, &self.base)?;
        if !credentials.validate() {
            return Err(unverified());
        }
        Ok(LoginOutcome {
            credentials,
            profile: Some(profile),
        })
    }
}

#[async_trait]
impl LoginProvider for HybridLoginProvider {
    async fn verify_session(
        &self,
        credentials: Credentials,
        budget: Duration,
    ) -> Result<Credentials, SafeError> {
        self.browser.verify_session(credentials, budget).await
    }
    async fn login(&self, username: String, password: String) -> Result<Credentials, SafeError> {
        self.login_context(username, password)
            .await
            .map(|o| o.credentials)
    }
    async fn login_context(
        &self,
        username: String,
        password: String,
    ) -> Result<LoginOutcome, SafeError> {
        tokio::time::timeout(self.timeout, self.attempt(username, password))
            .await
            .map_err(|_| SafeError::new("upstream_timeout"))?
    }
}

async fn bounded(response: reqwest::Response) -> Result<Vec<u8>, SafeError> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport)?;
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err(unexpected());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) fn recognized_waf(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    (text.contains("<html") || text.contains("<!doctype html") || text.contains("<script"))
        && (text.contains("acw_sc__v2")
            || (text.contains("checking your browser") && text.contains("<script"))
            || text.contains("cf-chl-")
            || text.contains("challenge-platform"))
}

fn extract(jar: &CookieStoreMutex, user: &str, base: &str) -> Result<Credentials, SafeError> {
    use cookie_store::{CookieDomain, CookieExpiration};
    let store = jar.lock().map_err(|_| unexpected())?;
    let cookies = store
        .iter_unexpired()
        .filter_map(|c| {
            let domain = match &c.domain {
                CookieDomain::HostOnly(d) => d.clone(),
                CookieDomain::Suffix(d) => format!(".{d}"),
                _ => return None,
            };
            // Test transport is private, never an operator-selectable upstream.
            #[cfg(test)]
            let domain = if base != UPSTREAM
                && domain.trim_start_matches('.') == reqwest::Url::parse(base).ok()?.host_str()?
            {
                "anyrouter.top".into()
            } else {
                domain
            };
            #[cfg(not(test))]
            let _ = base;
            if !matches!(domain.as_str(), "anyrouter.top" | ".anyrouter.top") {
                return None;
            }
            Some(Cookie {
                name: c.name().into(),
                value: c.value().into(),
                domain,
                path: c.path.as_ref().into(),
                secure: c.secure().unwrap_or(false),
                http_only: c.http_only().unwrap_or(false),
                expires: match c.expires {
                    CookieExpiration::SessionEnd => None,
                    CookieExpiration::AtUtc(t) => Some(t.unix_timestamp() as f64),
                },
            })
        })
        .collect();
    Ok(Credentials {
        cookies,
        api_user: user.into(),
    })
}
fn unavailable() -> SafeError {
    SafeError::new("upstream_unavailable")
}
fn unexpected() -> SafeError {
    SafeError::new("upstream_unexpected_response")
}
fn unverified() -> SafeError {
    SafeError::new("upstream_session_unverified").with_source(ErrorSource::SelfAccount)
}
fn transport(error: reqwest::Error) -> SafeError {
    SafeError::new(if error.is_timeout() {
        "upstream_timeout"
    } else {
        "upstream_unavailable"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::State,
        http::{HeaderMap, StatusCode, header},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Browser(AtomicUsize);
    #[async_trait]
    impl LoginProvider for Browser {
        async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Credentials {
                api_user: "7".into(),
                cookies: vec![Cookie {
                    name: "session".into(),
                    value: "local-fake-session".into(),
                    domain: "anyrouter.top".into(),
                    path: "/".into(),
                    secure: true,
                    http_only: true,
                    expires: None,
                }],
            })
        }
    }
    struct Mock {
        mode: &'static str,
        logins: AtomicUsize,
        profiles: AtomicUsize,
    }
    async fn login(
        State(s): State<Arc<Mock>>,
        headers: HeaderMap,
        axum::Json(input): axum::Json<Value>,
    ) -> Response {
        s.logins.fetch_add(1, Ordering::SeqCst);
        assert!(
            !headers.contains_key(header::COOKIE),
            "each attempt must start with an empty jar"
        );
        assert_eq!(input["password"], "local-fake-password");
        match s.mode {
            "timeout" => {
                tokio::time::sleep(Duration::from_secs(2)).await;
                "late".into_response()
            }
            "false" => axum::Json(
                serde_json::json!({"success":false,"message":"reflected-local-fake-password"}),
            )
            .into_response(),
            "unknown" => "<html><script>unknown page</script></html>".into_response(),
            "waf" => "<html><script>acw_sc__v2</script></html>".into_response(),
            "redirect" => (
                StatusCode::FOUND,
                [(header::LOCATION, "/api/user/self")],
                "<script>acw_sc__v2</script>",
            )
                .into_response(),
            "server" => (StatusCode::BAD_GATEWAY, "<script>acw_sc__v2</script>").into_response(),
            "large" => "x".repeat(256 * 1024 + 1).into_response(),
            "no_cookie" => {
                axum::Json(serde_json::json!({"success":true,"data":{"id":7}})).into_response()
            }
            _ => {
                let mut r =
                    axum::Json(serde_json::json!({"success":true,"data":{"id":7}})).into_response();
                for c in [
                    "session=local-fake-session; Path=/; HttpOnly",
                    "scoped=local-fake-scoped; Path=/api/user",
                    "irrelevant=local-fake-other; Path=/else",
                    "deleted=local-fake-old; Path=/",
                    "expired=gone; Max-Age=0; Path=/",
                    "foreign=ignored; Domain=invalid.example; Path=/",
                ] {
                    r.headers_mut()
                        .append(header::SET_COOKIE, c.parse().unwrap());
                }
                r
            }
        }
    }
    async fn profile(State(s): State<Arc<Mock>>, headers: HeaderMap) -> Response {
        s.profiles.fetch_add(1, Ordering::SeqCst);
        assert_eq!(headers["New-Api-User"], "7");
        let cookie = headers[header::COOKIE].to_str().unwrap();
        assert!(cookie.contains("session=local-fake-session"));
        assert!(cookie.contains("scoped=local-fake-scoped"));
        for name in ["irrelevant", "expired", "foreign"] {
            assert!(!cookie.contains(name));
        }
        let mut r = axum::Json(serde_json::json!({"success":true,"data":{"id":if s.mode == "mismatch" {8} else {7},"username":"confirmed-local-user","quota":"123","used_quota":4}})).into_response();
        r.headers_mut().append(
            header::SET_COOKIE,
            "deleted=; Max-Age=0; Path=/".parse().unwrap(),
        );
        r.headers_mut().append(
            header::SET_COOKIE,
            "session=local-fake-updated; Path=/; HttpOnly; Max-Age=60"
                .parse()
                .unwrap(),
        );
        r
    }

    #[tokio::test]
    async fn http_first_fallback_allowlist_and_fresh_cookie_jar() {
        for (mode, expected, browser_count, profile_count) in [
            ("ok", None, 0, 1),
            ("false", Some("upstream_login_failed"), 0, 0),
            ("unknown", Some("upstream_unexpected_response"), 0, 0),
            ("waf", None, 1, 0),
            ("redirect", Some("upstream_unavailable"), 0, 0),
            ("server", Some("upstream_unavailable"), 0, 0),
            ("large", Some("upstream_unexpected_response"), 0, 0),
            ("no_cookie", Some("upstream_session_unverified"), 0, 0),
            ("mismatch", Some("upstream_session_unverified"), 0, 1),
            ("timeout", Some("upstream_timeout"), 0, 0),
        ] {
            let state = Arc::new(Mock {
                mode,
                logins: AtomicUsize::new(0),
                profiles: AtomicUsize::new(0),
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let mock = state.clone();
            let task = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new()
                        .route("/api/user/login", post(login))
                        .route("/api/user/self", get(profile))
                        .with_state(mock),
                )
                .await
                .unwrap();
            });
            let browser = Arc::new(Browser(AtomicUsize::new(0)));
            let provider = HybridLoginProvider {
                base,
                browser: browser.clone(),
                timeout: Duration::from_millis(if mode == "timeout" { 100 } else { 3000 }),
            };
            for _ in 0..2 {
                let result = provider
                    .login_context("local-alias".into(), "local-fake-password".into())
                    .await;
                match (result, expected) {
                    (Err(error), Some(code)) => {
                        assert_eq!(error.code, code, "{mode}");
                        let text = serde_json::to_string(&error).unwrap();
                        assert!(!text.contains("local-fake"));
                    }
                    (Ok(outcome), None) => {
                        assert!(outcome.credentials.validate());
                        if mode == "ok" {
                            let profile = outcome.profile.unwrap();
                            assert_eq!(profile.1.as_deref(), Some("confirmed-local-user"));
                            assert_eq!(profile.0.quota_raw, "123");
                            assert!(
                                outcome
                                    .credentials
                                    .cookies
                                    .iter()
                                    .any(|c| c.name == "irrelevant" && c.path == "/else")
                            );
                            assert!(!outcome.credentials.cookies.iter().any(|c| matches!(
                                c.name.as_str(),
                                "deleted" | "expired" | "foreign"
                            )));
                            assert!(
                                outcome
                                    .credentials
                                    .cookies
                                    .iter()
                                    .any(|c| c.name == "session" && c.expires.is_some())
                            );
                        }
                    }
                    _ => panic!("unexpected safe outcome: {mode}"),
                }
            }
            assert_eq!(
                browser.0.load(Ordering::SeqCst),
                browser_count * 2,
                "{mode}"
            );
            assert_eq!(
                state.profiles.load(Ordering::SeqCst),
                profile_count * 2,
                "{mode}"
            );
            assert_eq!(state.logins.load(Ordering::SeqCst), 2, "{mode}");
            task.abort();
        }
    }

    #[tokio::test]
    async fn transport_failure_never_launches_browser() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let browser = Arc::new(Browser(AtomicUsize::new(0)));
        let provider = HybridLoginProvider {
            base,
            browser: browser.clone(),
            timeout: Duration::from_secs(1),
        };
        assert_eq!(
            provider
                .login("fake".into(), "fake".into())
                .await
                .err()
                .unwrap()
                .code,
            "upstream_unavailable"
        );
        assert_eq!(browser.0.load(Ordering::SeqCst), 0);
    }

    struct SlowBrowser {
        calls: AtomicUsize,
        budget_ms: AtomicUsize,
    }
    #[async_trait]
    impl LoginProvider for SlowBrowser {
        async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
            panic!("remaining budget must be supplied");
        }
        async fn login_with_budget(
            &self,
            _: String,
            _: String,
            budget: Duration,
        ) -> Result<Credentials, SafeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.budget_ms
                .store(budget.as_millis() as usize, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(5)).await;
            Err(SafeError::new("upstream_timeout"))
        }
    }
    #[tokio::test]
    async fn fallback_uses_remaining_total_deadline_and_only_one_browser() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/api/user/login",
                    post(|| async {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        "<html><script>acw_sc__v2</script></html>"
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let browser = Arc::new(SlowBrowser {
            calls: AtomicUsize::new(0),
            budget_ms: AtomicUsize::new(0),
        });
        let provider = HybridLoginProvider {
            base,
            browser: browser.clone(),
            timeout: Duration::from_millis(600),
        };
        let start = tokio::time::Instant::now();
        let error = provider
            .login("fake".into(), "fake".into())
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "upstream_timeout");
        assert_eq!(browser.calls.load(Ordering::SeqCst), 1);
        assert!(browser.budget_ms.load(Ordering::SeqCst) < 400);
        assert!(start.elapsed() < Duration::from_secs(1));
        task.abort();
    }
}
