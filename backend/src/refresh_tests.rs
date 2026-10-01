use super::*;
use crate::system_log::{EventReason, EventStage};

struct Verify {
    verified: Arc<AtomicBool>,
    calls: AtomicUsize,
    mismatch: bool,
}
#[async_trait]
impl LoginProvider for Verify {
    async fn login(&self, _: String, _: String) -> Result<Credentials, SafeError> {
        panic!("password login prohibited during refresh")
    }
    async fn verify_session(
        &self,
        mut c: Credentials,
        budget: Duration,
    ) -> Result<Credentials, SafeError> {
        assert!(budget > Duration::ZERO && budget < Duration::from_secs(90));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.verified.store(true, Ordering::SeqCst);
        c.cookies[0].value = "rotated-local-session".into();
        if self.mismatch {
            c.api_user = "8".into();
        }
        Ok(c)
    }
}

#[tokio::test]
async fn refresh_json_empty_body_contract_remains_strict_and_private() {
    let f = Fixture::new(true).await;
    let path = format!("/api/accounts/{}/refresh", f.account_id().await);
    assert_eq!(
        f.request("POST", &path, None, false, Some(json!({})))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = f.session().await;
    for input in [None, Some(json!({}))] {
        let response = f.request("POST", &path, Some(&cookie), false, input).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()["cache-control"], "no-store");
        f.done().await;
        assert_eq!(
            f.app
                .accounts
                .lock()
                .await
                .operation
                .as_ref()
                .unwrap()
                .status,
            "succeeded"
        );
    }
    for input in [
        json!({"password":"private-sentinel"}),
        json!(null),
        json!([]),
        json!(false),
    ] {
        let response = f
            .request("POST", &path, Some(&cookie), false, Some(input))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!json_body(response).await.to_string().contains("sentinel"));
    }
    assert_eq!(
        f.request(
            "POST",
            "/api/account/refresh",
            Some(&cookie),
            false,
            Some(json!({}))
        )
        .await
        .status(),
        StatusCode::GONE
    );
    let response = f
        .router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(&path)
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://untrusted.invalid")
                .header("cookie", &cookie)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()["cache-control"], "no-store");
    f.app.shutdown().await;
}

#[tokio::test]
async fn refresh_waf_only_once_and_atomic_snapshot_preserves_route_key_and_failures() {
    for mode in 0..12 {
        let verified = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let v = verified.clone();
        let n = calls.clone();
        let router = Router::new().fallback(any(move |request: Request| {
            let v = v.clone(); let n = n.clone();
            async move {
                assert_eq!(request.method(), "GET");
                let path = request.uri().path();
                assert!(matches!(path, "/api/user/self" | "/api/token/"));
                let verified = v.load(Ordering::SeqCst);
                assert_eq!(request.headers()["cookie"], format!("session={}", if verified { "rotated-local-session" } else { SECRET_COOKIE }));
                n.fetch_add(1, Ordering::SeqCst);
                if mode == 10 { tokio::time::sleep(Duration::from_millis(1500)).await; }
                let waf = mode == 2 || (!verified && (matches!(mode, 0 | 8 | 9) || (mode == 1 && path == "/api/token/")));
                let (status, body) = if waf {
                    (StatusCode::OK, b"<html><script>acw_sc__v2</script></html>".to_vec())
                } else if mode == 3 { (StatusCode::OK, br#"{"success":false,"message":"private-sentinel"}"#.to_vec()) }
                else if mode == 4 { (StatusCode::UNAUTHORIZED, b"{}".to_vec()) }
                else if mode == 5 { (StatusCode::BAD_GATEWAY, b"<script>acw_sc__v2</script>".to_vec()) }
                else if mode == 6 { (StatusCode::OK, b"<html>unknown-private-sentinel</html>".to_vec()) }
                else if mode == 7 { return (StatusCode::FOUND, [("location", "/api/user/self")], "<script>acw_sc__v2</script>").into_response(); }
                else if mode == 11 { (StatusCode::OK, br#"{"success":true,"data":{"id":7,"quota":false,"used_quota":3}}"#.to_vec()) }
                else if path == "/api/user/self" { (StatusCode::OK, br#"{"success":true,"data":{"id":7,"quota":"222","used_quota":"3"}}"#.to_vec()) }
                else { (StatusCode::OK, br#"{"success":true,"data":[{"id":1,"name":"existing","status":1,"key":"********"}]}"#.to_vec()) };
                (status, [("content-encoding", "gzip")], crate::upstream_body::gzip(&body)).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(Verify {
            verified,
            calls: AtomicUsize::new(0),
            mismatch: mode == 8,
        });
        let config = Config {
            root_key: ROOT.into(),
            state_path: dir.path().join("private/account.json"),
            helper_timeout: if mode == 10 { 1 } else { 90 },
            ..Default::default()
        };
        let app = App::new(
            config,
            model::Portfolio::from_active(active()),
            Upstream::mock(base),
            provider.clone(),
        );
        let old = app.accounts.lock().await.portfolio.clone();
        store::save(&app.config.state_path, &old).unwrap();
        if mode == 9 {
            app.fail_account_save.store(true, Ordering::SeqCst);
        }
        let entry = old.accounts[0].clone();
        let result = app::refresh_snapshot(&app, &entry).await;
        let result = match result {
            Ok(updated) => app::replace_entry(&app, &mut *app.accounts.lock().await, updated),
            Err(e) => Err(e),
        };
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            usize::from(matches!(mode, 0 | 1 | 2 | 8 | 9)),
            "mode {mode}"
        );
        if mode < 2 {
            assert!(result.is_ok());
            let state = app.accounts.lock().await;
            assert_eq!(state.portfolio.route_account_id, old.route_account_id);
            assert_eq!(
                state.portfolio.accounts[0]
                    .selected_key
                    .as_ref()
                    .unwrap()
                    .key,
                KEY
            );
            assert_eq!(
                state.portfolio.accounts[0].credentials.cookies[0].value,
                "rotated-local-session"
            );
            assert_eq!(state.portfolio.accounts[0].balance.quota_raw, "222");
            assert_eq!(
                serde_json::to_value(store::load(&app.config.state_path).unwrap()).unwrap(),
                serde_json::to_value(&state.portfolio).unwrap()
            );
        } else {
            let error = result.unwrap_err();
            let reason = match mode {
                2 => EventReason::KnownChallenge,
                3 => EventReason::JsonRejected,
                4 => EventReason::SessionExpired,
                5 => EventReason::HttpRejected,
                6 => EventReason::JsonInvalid,
                7 => EventReason::RedirectRejected,
                8 => EventReason::IdentityMismatch,
                9 => EventReason::StorageUnavailable,
                10 => EventReason::DeadlineExpired,
                _ => EventReason::QuotaInvalid,
            };
            assert_eq!(error.log_context().1, reason, "mode {mode}");
            assert_eq!(
                error.log_context().0,
                if mode == 9 {
                    EventStage::Storage
                } else {
                    EventStage::SelfAccount
                }
            );
            assert!(!serde_json::to_string(&error).unwrap().contains("sentinel"));
            assert_eq!(
                serde_json::to_value(&app.accounts.lock().await.portfolio).unwrap(),
                serde_json::to_value(&old).unwrap()
            );
            assert_eq!(
                serde_json::to_value(store::load(&app.config.state_path).unwrap()).unwrap(),
                serde_json::to_value(&old).unwrap()
            );
        }
        assert!(calls.load(Ordering::SeqCst) <= 4);
        app.shutdown().await;
        server.abort();
    }
}
