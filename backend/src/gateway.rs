use crate::{
    app::{App, require_root},
    error::ApiError,
};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri, header},
    response::Response,
};
use std::sync::Arc;

pub async fn proxy(State(app): State<Arc<App>>, request: Request) -> Result<Response, ApiError> {
    require_root(&app, request.headers())?;
    let target = target(&app.upstream.base, request.uri())?;
    let active =
        app.accounts.lock().await.active.clone().ok_or_else(|| {
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "account_not_configured")
        })?;
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    sanitize(&mut headers, true);
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
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "upstream_unavailable"))?;
    let status = upstream.status();
    let mut headers = upstream.headers().clone();
    sanitize(&mut headers, false);
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
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
