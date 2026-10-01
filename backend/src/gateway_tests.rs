//! Only loopback HTTP fixtures; production origin is never used.
use super::*;
use crate::{
    model_filter,
    request_log::{LogSink, MAX_BODY_BYTES},
};
use futures_util::StreamExt;
use std::sync::Mutex;

struct GatewayMock {
    status: Mutex<StatusCode>,
    bytes: Mutex<Vec<u8>>,
    headers: Mutex<HeaderMap>,
    received: Mutex<Vec<(String, HeaderMap, Vec<u8>)>>,
    hold: AtomicBool,
    release: Arc<tokio::sync::Notify>,
}
async fn upstream(State(state): State<Arc<GatewayMock>>, request: Request) -> Response {
    let uri = request.uri().to_string();
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    state
        .received
        .lock()
        .unwrap()
        .push((uri, headers, body.to_vec()));
    let bytes = state.bytes.lock().unwrap().clone();
    let first =
        futures_util::stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(bytes)) });
    let hold = state.hold.load(Ordering::SeqCst);
    let release = state.release.clone();
    let last = futures_util::stream::once(async move {
        if hold {
            release.notified().await;
        }
        Ok::<_, std::io::Error>(Bytes::new())
    });
    let mut response = Response::new(Body::from_stream(first.chain(last)));
    *response.status_mut() = *state.status.lock().unwrap();
    *response.headers_mut() = state.headers.lock().unwrap().clone();
    response
}
struct Harness {
    app: Arc<App>,
    router: Router,
    state: Arc<GatewayMock>,
    task: tokio::task::JoinHandle<()>,
    directory: tempfile::TempDir,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Harness {
    async fn new(status: StatusCode, bytes: &[u8]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            root_key: ROOT.into(),
            state_path: directory.path().join("private/account.json"),
            ..Config::default()
        };
        let sink = LogSink::open(config.request_log_path()).await.unwrap();
        let state = Arc::new(GatewayMock {
            status: Mutex::new(status),
            bytes: Mutex::new(bytes.into()),
            headers: Mutex::new(HeaderMap::from_iter([(
                header::CONTENT_TYPE,
                "application/json".parse().unwrap(),
            )])),
            received: Mutex::new(Vec::new()),
            hold: AtomicBool::new(false),
            release: Arc::new(tokio::sync::Notify::new()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mock = Router::new()
            .fallback(any(upstream))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, mock).await.unwrap();
        });
        let app = App::new_with_logs(
            config,
            model::Portfolio::from_active(active()),
            Upstream::mock(base),
            Arc::new(FakeLogin {
                fail: AtomicBool::new(false),
            }),
            Some(sink),
        );
        Self {
            router: app::router(app.clone()),
            app,
            state,
            task,
            directory,
        }
    }
    async fn send(
        &self,
        method: &str,
        path: &str,
        extra: &[(&str, &str)],
        body: &[u8],
    ) -> Response {
        let mut r = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:8080")
            .header("authorization", format!("Bearer {ROOT}"));
        for (k, v) in extra {
            r = r.header(*k, *v);
        }
        self.router
            .clone()
            .oneshot(r.body(Body::from(body.to_vec())).unwrap())
            .await
            .unwrap()
    }
    async fn entries(&self) -> Vec<crate::request_log::LogEntry> {
        let logs = self.app.logs.as_ref().unwrap();
        logs.flush().await.unwrap();
        logs.page(Some(1000)).unwrap().items
    }
    async fn session(&self) -> String {
        let r = self
            .send(
                "POST",
                "/api/admin/session",
                &[("origin", "http://localhost:5173")],
                b"",
            )
            .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        r.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .into()
    }
}

#[tokio::test]
async fn successful_sse_metadata_is_logged_at_headers_without_waiting_for_eof() {
    let h = Harness::new(StatusCode::OK, b"data: early\n\n").await;
    h.state.hold.store(true, Ordering::SeqCst);
    h.state
        .headers
        .lock()
        .unwrap()
        .insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    let r = tokio::time::timeout(
        Duration::from_secs(2),
        h.send("GET", "/v1/events", &[], b""),
    )
    .await
    .unwrap();
    let entries = h.entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].http_status, Some(200));
    assert_eq!(entries[0].error_body, None);
    assert!(!entries[0].truncated);
    let mut stream = r.into_body().into_data_stream();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "data: early\n\n"
    );
    drop(stream);
    assert_eq!(h.entries().await.len(), 1);
    h.app.shutdown_logs().await;
}

#[tokio::test]
async fn clear_during_error_stream_blocks_old_eof_and_drop_without_changing_bytes() {
    let h = Harness::new(StatusCode::BAD_REQUEST, b"original-stream-FAKE").await;
    let logs = h.app.logs.as_ref().unwrap();
    for eof in [true, false] {
        h.state.hold.store(true, Ordering::SeqCst);
        let r = h.send("GET", "/v1/error", &[], b"").await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let mut stream = r.into_body().into_data_stream();
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            "original-stream-FAKE"
        );
        logs.clear().await.unwrap();
        if eof {
            h.state.release.notify_one();
            while let Some(chunk) = stream.next().await {
                assert!(chunk.unwrap().is_empty());
            }
        }
        drop(stream);
        assert!(h.entries().await.is_empty());
    }
    h.state.hold.store(false, Ordering::SeqCst);
    let r = h.send("GET", "/v1/error", &[], b"").await;
    assert_eq!(
        to_bytes(r.into_body(), 1024).await.unwrap(),
        "original-stream-FAKE"
    );
    let entries = h.entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].error_body.as_deref(),
        Some("original-stream-FAKE")
    );
    assert!(!entries[0].truncated);
    h.app.shutdown_logs().await;
}

#[tokio::test]
async fn error_raw_prefix_eof_large_body_and_drop_are_logged_once() {
    let raw = b"FAKE-key=raw-secret\n\xff bytes";
    let h = Harness::new(StatusCode::FORBIDDEN, raw).await;
    let r = h.send("GET", "/v1/error", &[], b"").await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(to_bytes(r.into_body(), 1024).await.unwrap(), raw.as_slice());
    let entries = h.entries().await;
    assert_eq!(
        entries[0].error_body.as_deref(),
        Some(String::from_utf8_lossy(raw).as_ref())
    );
    assert!(!entries[0].truncated);
    *h.state.bytes.lock().unwrap() = vec![b'x'; MAX_BODY_BYTES + 100];
    let r = h.send("GET", "/v1/error", &[], b"").await;
    assert_eq!(
        to_bytes(r.into_body(), MAX_BODY_BYTES * 2)
            .await
            .unwrap()
            .len(),
        MAX_BODY_BYTES + 100
    );
    let entries = h.entries().await;
    assert_eq!(entries[0].error_body, Some("x".repeat(MAX_BODY_BYTES)));
    assert!(entries[0].truncated);
    *h.state.bytes.lock().unwrap() = b"short original".to_vec();
    h.state.hold.store(true, Ordering::SeqCst);
    let r = h.send("GET", "/v1/error", &[], b"").await;
    let mut stream = r.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap(), "short original");
    drop(stream);
    let entries = h.entries().await;
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].error_body.as_deref(), Some("short original"));
    assert!(entries[0].truncated);
    let r = h.send("GET", "/v1/error", &[], b"").await;
    drop(r);
    let entries = h.entries().await;
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].error_body.as_deref(), Some(""));
    assert!(entries[0].truncated);
    h.app.shutdown_logs().await;
}

#[tokio::test]
async fn actual_tcp_error_client_abort_preserves_short_partial_log() {
    let h = Harness::new(StatusCode::BAD_REQUEST, b"partial-FAKE").await;
    h.state.hold.store(true, Ordering::SeqCst);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = h.router.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    socket.write_all(format!("GET /v1/error HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nAuthorization: Bearer {ROOT}\r\n\r\n").as_bytes()).await.unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !received.windows(12).any(|w| w == b"partial-FAKE") {
            let mut b = [0; 512];
            let n = socket.read(&mut b).await.unwrap();
            assert!(n > 0);
            received.extend_from_slice(&b[..n]);
        }
    })
    .await
    .unwrap();
    drop(socket);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let entries = h.entries().await;
            if !entries.is_empty() {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].error_body.as_deref(), Some("partial-FAKE"));
                assert!(entries[0].truncated);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    h.app.shutdown_logs().await;
}

#[tokio::test]
async fn upstream_stream_error_keeps_original_prefix_and_finishes_once() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        root_key: ROOT.into(),
        state_path: directory.path().join("private/account.json"),
        ..Config::default()
    };
    let logs = LogSink::open(config.request_log_path()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (release, wait) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut bytes = [0; 512];
            let n = socket.read(&mut bytes).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&bytes[..n]);
        }
        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\nContent-Type: text/plain\r\n\r\nshort-stream-error").await.unwrap();
        let _ = wait.await;
        // Deliberately terminate before Content-Length; no real upstream.
    });
    let app = App::new_with_logs(
        config,
        model::Portfolio::from_active(active()),
        Upstream::mock(base),
        Arc::new(FakeLogin {
            fail: AtomicBool::new(false),
        }),
        Some(logs),
    );
    let request = axum::http::Request::builder()
        .uri("/v1/error")
        .header("host", "127.0.0.1:8080")
        .header("authorization", format!("Bearer {ROOT}"))
        .body(Body::empty())
        .unwrap();
    let response = app::router(app.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap(), "short-stream-error");
    release.send(()).unwrap();
    assert!(stream.next().await.unwrap().is_err());
    drop(stream);
    app.logs.as_ref().unwrap().flush().await.unwrap();
    let items = app.logs.as_ref().unwrap().page(None).unwrap().items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].http_status, Some(503));
    assert_eq!(items[0].error_body.as_deref(), Some("short-stream-error"));
    assert!(items[0].truncated);
    task.await.unwrap();
    app.shutdown_logs().await;
}

#[tokio::test]
async fn route_switch_keeps_old_response_identity_and_orders_by_request_time() {
    let h = Harness::new(StatusCode::BAD_REQUEST, b"original-A").await;
    h.state.hold.store(true, Ordering::SeqCst);
    let a = h.app.accounts.lock().await.portfolio.accounts[0].clone();
    let r = h.send("GET", "/v1/error", &[], b"").await;
    let mut old = r.into_body().into_data_stream();
    assert_eq!(old.next().await.unwrap().unwrap(), "original-A");
    let mut b = a.clone();
    b.id = model::id();
    b.upstream_user_id = "8".into();
    b.username = "account-B".into();
    b.credentials.api_user = "8".into();
    b.credentials.cookies[0].value = COOKIE_B.into();
    b.selected_key.as_mut().unwrap().key = KEY_B.into();
    {
        let mut accounts = h.app.accounts.lock().await;
        accounts.portfolio.accounts.push(b.clone());
    }
    let cookie = h.session().await;
    let route = h
        .send(
            "POST",
            &format!("/api/accounts/{}/route", b.id),
            &[
                ("cookie", &cookie),
                ("origin", "http://localhost:5173"),
                ("content-type", "application/json"),
            ],
            serde_json::to_vec(&json!({"revision":b.revision}))
                .unwrap()
                .as_slice(),
        )
        .await;
    assert_eq!(route.status(), StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(2)).await;
    *h.state.status.lock().unwrap() = StatusCode::OK;
    h.state.hold.store(false, Ordering::SeqCst);
    let r = h.send("GET", "/v1/new", &[], b"").await;
    to_bytes(r.into_body(), 1024).await.unwrap();
    h.state.release.notify_one();
    while let Some(c) = old.next().await {
        c.unwrap();
    }
    let entries = h.entries().await;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].account_id.as_deref(), Some(b.id.as_str()));
    assert_eq!(entries[0].account_name.as_deref(), Some("account-B"));
    assert_eq!(entries[1].account_id.as_deref(), Some(a.id.as_str()));
    assert_eq!(entries[1].error_body.as_deref(), Some("original-A"));
    {
        let received = h.state.received.lock().unwrap();
        assert_eq!(
            received[0].1[header::AUTHORIZATION],
            format!("Bearer {KEY}")
        );
        assert_eq!(
            received[1].1[header::AUTHORIZATION],
            format!("Bearer {KEY_B}")
        );
    }
    h.app.shutdown_logs().await;
}

#[tokio::test]
async fn logs_session_query_no_route_transport_and_private_shutdown_reload() {
    let h = Harness::new(StatusCode::OK, b"not retained").await;
    let r = axum::http::Request::builder()
        .uri("/v1/test")
        .header("host", "127.0.0.1:8080")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        h.router.clone().oneshot(r).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(h.entries().await.is_empty());
    let denied = h.send("GET", "/api/request-logs", &[], b"").await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
    let cookie = h.session().await;
    for suffix in [
        "?limit=0",
        "?limit=-1",
        "?limit=abc",
        "?limit=1&limit=2",
        "?other=1",
        "?limit=999999999999999999999999",
    ] {
        assert_eq!(
            h.send(
                "GET",
                &format!("/api/request-logs{suffix}"),
                &[("cookie", &cookie)],
                b""
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let (snapshot, route) = {
        let mut accounts = h.app.accounts.lock().await;
        (
            accounts.active.take(),
            accounts.portfolio.route_account_id.take(),
        )
    };
    assert_eq!(
        h.send("GET", "/v1/test", &[], b"").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(h.entries().await[0].http_status, None);
    assert_eq!(h.entries().await[0].account_id, None);
    {
        let mut accounts = h.app.accounts.lock().await;
        accounts.active = snapshot;
        accounts.portfolio.route_account_id = route;
    }
    h.task.abort();
    tokio::task::yield_now().await;
    let r = h.send("GET", "/v1/test", &[], b"").await;
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(h.entries().await[0].http_status, None);
    let r = h
        .send(
            "GET",
            "/api/request-logs?limit=2000",
            &[("cookie", &cookie)],
            b"",
        )
        .await;
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
    let dto = json_body(r).await;
    assert_eq!(dto["items"].as_array().unwrap().len(), 2);
    assert_eq!(dto["dropped_count"], 0);
    h.app.shutdown().await;
    h.app.shutdown_logs().await;
    let path = h.app.config.request_log_path();
    assert!(path.starts_with(h.directory.path()));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let reopened = LogSink::open(path).await.unwrap();
    assert_eq!(reopened.page(None).unwrap().items.len(), 2);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn models_exact_filter_raw_metadata_headers_and_unmodified_other_requests() {
    let mut body = String::from(r#" {"z":1.00,"data":["#);
    for id in model_filter::DENIED_IDS {
        body.push_str(&format!(r#"{{"id":"{id}"}},"#));
    }
    body.push_str(r#"{"z":1e+02,"id":"GPT-5-CODEX","name":"gpt-5-codex"},{"id":"gpt-5-codex-extra"},{"id":"safe"},{"id":"safe"}],"tail":{"b":2,"a":1}} "#);
    let h = Harness::new(StatusCode::OK, body.as_bytes()).await;
    for name in [
        "etag",
        "digest",
        "content-digest",
        "repr-digest",
        "content-md5",
        "content-range",
        "last-modified",
        "vary",
        "accept-ranges",
    ] {
        h.state.headers.lock().unwrap().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            "fixture".parse().unwrap(),
        );
    }
    let extra = [
        ("accept-encoding", "gzip"),
        ("if-none-match", "tag"),
        ("if-modified-since", "date"),
        ("range", "bytes=0-1"),
        ("if-range", "tag"),
    ];
    let r = h.send("GET", "/v1/models?q=%2F", &extra, b"").await;
    assert_eq!(r.status(), StatusCode::OK);
    for name in [
        "etag",
        "digest",
        "content-digest",
        "repr-digest",
        "content-md5",
        "content-range",
        "last-modified",
        "vary",
        "accept-ranges",
    ] {
        assert!(!r.headers().contains_key(name), "stale header: {name}");
    }
    let bytes = to_bytes(r.into_body(), model_filter::MAX_MODEL_BYTES)
        .await
        .unwrap();
    assert_eq!(
        bytes.as_ref(),
        model_filter::filter_models(body.as_bytes()).unwrap().bytes
    );
    let dto: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(dto["data"].as_array().unwrap().len(), 4);
    assert!(String::from_utf8(bytes.to_vec()).unwrap().contains("1e+02"));
    {
        let received = h.state.received.lock().unwrap();
        assert_eq!(received[0].0, "/v1/models?q=%2F");
        assert_eq!(received[0].1[header::ACCEPT_ENCODING], "identity");
        for name in ["if-none-match", "if-modified-since", "range", "if-range"] {
            assert!(!received[0].1.contains_key(name));
        }
    }
    let payload = br#"{"model":"gpt-5-codex","messages":[]}"#;
    let r = h
        .send("POST", "/v1/chat/completions", &extra, payload)
        .await;
    for name in ["content-digest", "repr-digest"] {
        assert_eq!(r.headers()[name], "fixture");
    }
    assert_eq!(
        to_bytes(r.into_body(), model_filter::MAX_MODEL_BYTES)
            .await
            .unwrap()
            .as_ref(),
        body.as_bytes()
    );
    let r = h.send("HEAD", "/v1/models", &extra, b"").await;
    assert_eq!(r.status(), StatusCode::OK);
    for name in ["content-digest", "repr-digest"] {
        assert_eq!(r.headers()[name], "fixture");
    }
    drop(r);
    {
        let received = h.state.received.lock().unwrap();
        for request in &received[1..] {
            for (name, value) in extra {
                assert_eq!(request.1[name], value);
            }
        }
        assert_eq!(received[1].2, payload);
    }
    assert!(
        h.entries()
            .await
            .iter()
            .all(|e| e.http_status == Some(200) && e.error_body.is_none())
    );
    h.app.shutdown_logs().await;
}

#[tokio::test]
async fn models_invalid_encoding_type_shape_size_and_non200_raw_error() {
    let h = Harness::new(StatusCode::OK, br#"{"data":[]}"#).await;
    for encoding in ["gzip", "br", "deflate"] {
        h.state
            .headers
            .lock()
            .unwrap()
            .insert(header::CONTENT_ENCODING, encoding.parse().unwrap());
        let r = h.send("GET", "/v1/models", &[], b"").await;
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            json_body(r).await["error"]["code"],
            "models_response_invalid"
        );
    }
    h.state
        .headers
        .lock()
        .unwrap()
        .remove(header::CONTENT_ENCODING);
    for (content_type, bytes, code) in [
        (
            "text/html",
            b"<html>fixture</html>".to_vec(),
            "models_response_invalid",
        ),
        (
            "application/json",
            b"{bad".to_vec(),
            "models_response_invalid",
        ),
        (
            "application/json",
            b"{}".to_vec(),
            "models_response_invalid",
        ),
        (
            "application/json",
            vec![b' '; model_filter::MAX_MODEL_BYTES + 1],
            "models_response_too_large",
        ),
    ] {
        *h.state.bytes.lock().unwrap() = bytes;
        h.state
            .headers
            .lock()
            .unwrap()
            .insert(header::CONTENT_TYPE, content_type.parse().unwrap());
        let r = h.send("GET", "/v1/models", &[], b"").await;
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(json_body(r).await["error"]["code"], code);
    }
    let entries = h.entries().await;
    assert_eq!(entries.len(), 7);
    assert!(
        entries
            .iter()
            .all(|e| e.http_status == Some(200) && e.error_body.is_none() && !e.truncated)
    );
    *h.state.status.lock().unwrap() = StatusCode::BAD_REQUEST;
    *h.state.bytes.lock().unwrap() = b"original non-json upstream error".to_vec();
    h.state
        .headers
        .lock()
        .unwrap()
        .insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
    let r = h.send("GET", "/v1/models", &[], b"").await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        to_bytes(r.into_body(), 1024).await.unwrap(),
        "original non-json upstream error"
    );
    assert_eq!(
        h.entries().await[0].error_body.as_deref(),
        Some("original non-json upstream error")
    );
    *h.state.status.lock().unwrap() = StatusCode::OK;
    *h.state.bytes.lock().unwrap() = br#"{"data":[]}"#.to_vec();
    h.state.headers.lock().unwrap().insert(
        header::CONTENT_TYPE,
        "application/vnd.fixture+json; charset=utf-8"
            .parse()
            .unwrap(),
    );
    let r = h.send("GET", "/v1/models", &[], b"").await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(r.into_body(), 1024).await.unwrap(),
        br#"{"data":[]}"#.as_slice()
    );
    h.app.shutdown_logs().await;
}

#[tokio::test]
async fn failed_log_persistence_cannot_change_successful_gateway_status() {
    let h = Harness::new(StatusCode::OK, b"success body").await;
    std::fs::create_dir_all(h.app.config.request_log_path()).unwrap();
    let r = h
        .send("POST", "/v1/chat/completions", &[], b"unchanged")
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(to_bytes(r.into_body(), 1024).await.unwrap(), "success body");
    let logs = h.app.logs.as_ref().unwrap();
    assert!(logs.flush().await.is_err());
    assert!(logs.failed());
    let cookie = h.session().await;
    let r = h
        .send("GET", "/api/request-logs", &[("cookie", &cookie)], b"")
        .await;
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(r).await["error"]["code"], "request_log_failed");
    h.app.shutdown_logs().await;
}
