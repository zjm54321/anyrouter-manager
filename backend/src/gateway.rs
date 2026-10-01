use crate::{
    app::{App, require_root},
    error::ApiError,
    model_filter::{FilterError, MAX_MODEL_BYTES, filter_models},
    request_log::{ErrorBodyCapture, LogEntry, LogSink},
};
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri, header},
    response::Response,
};
use futures_util::{Stream, StreamExt};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

struct LoggedStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    pending: Option<(LogEntry, ErrorBodyCapture)>,
    sink: Option<LogSink>,
    generation: u64,
}
impl LoggedStream {
    fn finish(&mut self, incomplete: bool) {
        if let Some((mut entry, mut capture)) = self.pending.take() {
            if incomplete {
                capture.mark_incomplete();
            }
            entry.set_response(entry.http_status, capture);
            if let Some(sink) = &self.sink {
                sink.try_enqueue_for(self.generation, entry);
            }
        }
    }
}
impl Stream for LoggedStream {
    type Item = Result<Bytes, reqwest::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let next = self.inner.as_mut().poll_next(cx);
        match &next {
            Poll::Ready(Some(Ok(bytes))) => {
                if let Some((_, capture)) = &mut self.pending {
                    capture.push(bytes);
                }
            }
            Poll::Ready(Some(Err(_))) => self.finish(true),
            Poll::Ready(None) => self.finish(false),
            Poll::Pending => {}
        }
        next
    }
}
impl Drop for LoggedStream {
    fn drop(&mut self) {
        self.finish(true);
    }
}

fn enqueue(sink: &Option<LogSink>, generation: u64, entry: LogEntry) {
    if let Some(sink) = sink {
        sink.try_enqueue_for(generation, entry);
    }
}

pub async fn proxy(State(app): State<Arc<App>>, request: Request) -> Result<Response, ApiError> {
    require_root(&app, request.headers())?;
    let generation = app.logs.as_ref().map_or(0, LogSink::generation);
    let (active, mut entry) = {
        let accounts = app.accounts.lock().await;
        let route = accounts.portfolio.route();
        (
            accounts.active.clone(),
            LogEntry::new(
                route.map(|a| a.id.clone()),
                route.map(|a| a.username.clone()),
            ),
        )
    };
    let Some(active) = active else {
        enqueue(&app.logs, generation, entry);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "account_not_configured",
        ));
    };
    let target = target(&app.upstream.base, request.uri())?;
    let models =
        request.method() == axum::http::Method::GET && request.uri().path() == "/v1/models";
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    sanitize(&mut headers, true);
    if models {
        headers.insert(
            header::ACCEPT_ENCODING,
            axum::http::HeaderValue::from_static("identity"),
        );
        for name in [
            header::IF_NONE_MATCH,
            header::IF_MODIFIED_SINCE,
            header::RANGE,
            header::IF_RANGE,
        ] {
            headers.remove(name);
        }
    }
    let authorization = format!("Bearer {}", active.key)
        .parse()
        .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "account_not_configured"))?;
    headers.insert(header::AUTHORIZATION, authorization);
    let upstream = app
        .upstream
        .client
        .request(parts.method, target)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await;
    let upstream = match upstream {
        Ok(upstream) => upstream,
        Err(_) => {
            enqueue(&app.logs, generation, entry);
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "upstream_unavailable",
            ));
        }
    };
    let status = upstream.status();
    entry.http_status = Some(status.as_u16());
    let capture = ErrorBodyCapture::new(entry.http_status);
    let pending = if capture.enabled() {
        Some((entry, capture))
    } else {
        enqueue(&app.logs, generation, entry);
        None
    };
    let mut headers = upstream.headers().clone();
    sanitize(&mut headers, false);
    let body = if models && status == StatusCode::OK {
        let invalid =
            || ApiError::new(StatusCode::BAD_GATEWAY, FilterError::InvalidResponse.code());
        if upstream
            .headers()
            .get_all(header::CONTENT_ENCODING)
            .iter()
            .any(|v| {
                !v.to_str()
                    .is_ok_and(|s| s.trim().eq_ignore_ascii_case("identity"))
            })
        {
            return Err(invalid());
        }
        let json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| {
                s.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            });
        if !json.is_some_and(|s| {
            s == "application/json" || (s.starts_with("application/") && s.ends_with("+json"))
        }) {
            return Err(invalid());
        }
        let mut stream = upstream.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| invalid())?;
            if chunk.len() > MAX_MODEL_BYTES - bytes.len() {
                return Err(ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    FilterError::TooLarge.code(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let filtered = filter_models(&bytes)
            .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, error.code()))?;
        for name in [
            "content-length",
            "etag",
            "digest",
            "content-digest",
            "repr-digest",
            "content-md5",
            "content-range",
            "last-modified",
            "vary",
            "content-encoding",
            "accept-ranges",
        ] {
            headers.remove(name);
        }
        Body::from(filtered.bytes)
    } else {
        Body::from_stream(LoggedStream {
            inner: Box::pin(upstream.bytes_stream()),
            pending,
            sink: app.logs.clone(),
            generation,
        })
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

pub(crate) fn target(base: &str, uri: &Uri) -> Result<reqwest::Url, ApiError> {
    let bad = || ApiError::new(StatusCode::BAD_REQUEST, "bad_request");
    if uri.scheme().is_some() || uri.authority().is_some() {
        return Err(bad());
    }
    let path = uri.path();
    if path != "/v1" && !path.starts_with("/v1/") {
        return Err(bad());
    }
    let mut decoded = Vec::new();
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = path.get(index + 1..index + 3).ok_or_else(bad)?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| bad())?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    if decoded.contains(&b'\\')
        || decoded.windows(2).any(|p| p == b"//")
        || decoded
            .split(|b| *b == b'/')
            .any(|segment| segment == b"." || segment == b"..")
        || decoded.iter().any(|b| *b < 0x20 || *b == 0x7f)
    {
        return Err(bad());
    }
    let raw = uri.path_and_query().ok_or_else(bad)?.as_str();
    let url = reqwest::Url::parse(&format!("{base}{raw}")).map_err(|_| bad())?;
    let original = reqwest::Url::parse(base).map_err(|_| bad())?;
    if url.origin() != original.origin() || url.path() != path || url.query() != uri.query() {
        return Err(bad());
    }
    Ok(url)
}

pub(crate) fn sanitize(headers: &mut HeaderMap, request: bool) {
    let connection_headers = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|v| v.trim().to_owned())
        .collect::<Vec<_>>();
    for name in connection_headers {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "trailers",
        "transfer-encoding",
        "upgrade",
        "authorization",
        "cookie",
        "set-cookie",
        "new-api-user",
        "x-api-key",
        "x-root-key",
        "root-key",
        "x-session-token",
    ] {
        headers.remove(name);
    }
    let private = headers
        .keys()
        .filter(|name| {
            let name = name.as_str();
            name.starts_with("x-admin-")
                || name.starts_with("x-management-")
                || name.starts_with("x-forwarded-")
                || name == "forwarded"
                || name == "proxy-connection"
        })
        .cloned()
        .collect::<Vec<_>>();
    for name in private {
        headers.remove(name);
    }
    if request {
        headers.remove(header::HOST);
        headers.remove(header::ORIGIN);
        headers.remove(header::REFERER);
    }
}
