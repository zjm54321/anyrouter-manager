//! Bounded decoding for management JSON/WAF responses, never gateway streams.
//! Some upstream edges gzip even when the client did not advertise compression.
use crate::error::SafeError;
use crate::system_log::EventReason;
use futures_util::StreamExt;
use std::io::Read;

pub(crate) const MANAGEMENT_LIMIT: usize = 256 * 1024;

pub(crate) fn recognized_waf(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    (text.contains("<html") || text.contains("<!doctype html") || text.contains("<script"))
        && (text.contains("acw_sc__v2")
            || (text.contains("checking your browser") && text.contains("<script"))
            || text.contains("cf-chl-")
            || text.contains("challenge-platform"))
}

/// Management endpoints only. Redirects/5xx/JSON refusals never become a browser gate.
pub(crate) async fn json(
    response: reqwest::Response,
) -> Result<(reqwest::StatusCode, serde_json::Value), SafeError> {
    let status = response.status();
    if status.is_server_error() {
        return Err(SafeError::new("upstream_unavailable").with_reason(EventReason::HttpRejected));
    }
    if status.is_redirection() {
        return Err(unexpected().with_reason(EventReason::RedirectRejected));
    }
    let bytes = bounded(response, MANAGEMENT_LIMIT).await?;
    let parsed = serde_json::from_slice::<serde_json::Value>(&bytes);
    if parsed.is_err() && recognized_waf(&bytes) {
        return Err(SafeError::new("upstream_challenge").with_reason(EventReason::KnownChallenge));
    }
    if matches!(status.as_u16(), 401 | 403) {
        return Err(
            SafeError::new("upstream_session_expired").with_reason(EventReason::SessionExpired)
        );
    }
    let value = parsed.map_err(|_| unexpected().with_reason(EventReason::JsonInvalid))?;
    if !value.is_object() {
        return Err(unexpected().with_reason(EventReason::SchemaInvalid));
    }
    Ok((status, value))
}

pub(crate) fn data(
    status: reqwest::StatusCode,
    value: serde_json::Value,
) -> Result<serde_json::Value, SafeError> {
    if !status.is_success() {
        return Err(unexpected().with_reason(EventReason::HttpRejected));
    }
    match value.get("success").and_then(serde_json::Value::as_bool) {
        Some(true) => value
            .get("data")
            .cloned()
            .ok_or_else(|| unexpected().with_reason(EventReason::SchemaInvalid)),
        Some(false) => Err(unexpected().with_reason(EventReason::JsonRejected)),
        None => Err(unexpected().with_reason(EventReason::SchemaInvalid)),
    }
}

pub(crate) async fn bounded(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, SafeError> {
    let encodings: Vec<_> = response
        .headers()
        .get_all(reqwest::header::CONTENT_ENCODING)
        .iter()
        .collect();
    let gzip = match encodings.as_slice() {
        [] => false,
        [value] if value.as_bytes().eq_ignore_ascii_case(b"identity") => false,
        [value] if value.as_bytes().eq_ignore_ascii_case(b"gzip") => true,
        _ => return Err(unexpected().with_reason(EventReason::BodyEncoding)),
    };
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            SafeError::new(if error.is_timeout() {
                "upstream_timeout"
            } else {
                "upstream_unavailable"
            })
        })?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(unexpected().with_reason(EventReason::BodyLimit));
        }
        bytes.extend_from_slice(&chunk);
    }
    decode(bytes, gzip, limit)
}

fn decode(bytes: Vec<u8>, gzip: bool, limit: usize) -> Result<Vec<u8>, SafeError> {
    if !gzip {
        return Ok(bytes);
    }
    let mut decoded = Vec::new();
    flate2::read::MultiGzDecoder::new(bytes.as_slice())
        .take(limit as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|_| unexpected().with_reason(EventReason::BodyDecode))?;
    if decoded.len() > limit {
        return Err(unexpected().with_reason(EventReason::BodyLimit));
    }
    Ok(decoded)
}

fn unexpected() -> SafeError {
    SafeError::new("upstream_unexpected_response")
}

#[cfg(test)]
pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn management_json_classifies_bounded_waf_refusal_redirect_and_encoding() {
        use axum::{Router, http::StatusCode, routing::get};
        for (status, coding, body, expected) in [
            (
                200,
                "gzip",
                gzip(b"<script>acw_sc__v2</script>"),
                Some(EventReason::KnownChallenge),
            ),
            (
                403,
                "gzip",
                gzip(b"<html>cf-chl-</html>"),
                Some(EventReason::KnownChallenge),
            ),
            (
                200,
                "gzip",
                gzip(b"<html>unknown-private-sentinel</html>"),
                Some(EventReason::JsonInvalid),
            ),
            (
                401,
                "identity",
                b"{}".to_vec(),
                Some(EventReason::SessionExpired),
            ),
            (
                200,
                "identity",
                br#"{"success":false,"message":"private-sentinel"}"#.to_vec(),
                Some(EventReason::JsonRejected),
            ),
            (200, "gzip", gzip(br#"{"success":true,"data":{}}"#), None),
            (
                502,
                "gzip",
                gzip(b"<script>acw_sc__v2</script>"),
                Some(EventReason::HttpRejected),
            ),
            (
                302,
                "gzip",
                gzip(b"<script>acw_sc__v2</script>"),
                Some(EventReason::RedirectRejected),
            ),
            (200, "br", b"{}".to_vec(), Some(EventReason::BodyEncoding)),
            (
                200,
                "gzip",
                b"invalid-private-sentinel".to_vec(),
                Some(EventReason::BodyDecode),
            ),
            (
                200,
                "gzip",
                gzip(&vec![b'x'; MANAGEMENT_LIMIT + 1]),
                Some(EventReason::BodyLimit),
            ),
            (
                200,
                "identity",
                vec![b'x'; MANAGEMENT_LIMIT + 1],
                Some(EventReason::BodyLimit),
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new().route(
                        "/",
                        get(move || async move {
                            (
                                StatusCode::from_u16(status).unwrap(),
                                [("content-encoding", coding)],
                                body,
                            )
                        }),
                    ),
                )
                .await
                .unwrap();
            });
            let response = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
                .get(format!("http://{addr}/"))
                .send()
                .await
                .unwrap();
            let result = match json(response).await {
                Ok((status, value)) => data(status, value),
                Err(e) => Err(e),
            };
            match expected {
                None => assert!(result.is_ok()),
                Some(reason) => {
                    let error = result.unwrap_err();
                    assert_eq!(error.reason, Some(reason));
                    assert!(!serde_json::to_string(&error).unwrap().contains("sentinel"));
                }
            }
            server.abort();
        }
    }

    #[test]
    fn gzip_is_bounded_and_requires_complete_valid_stream() {
        let bytes = br#"{"success":true,"data":{"id":7}}"#;
        let compressed = gzip(bytes);
        assert_eq!(
            decode(compressed.clone(), true, bytes.len()).unwrap(),
            bytes
        );
        for invalid in [
            compressed[..compressed.len() - 1].to_vec(),
            [compressed.clone(), b"trailing-secret-sentinel".to_vec()].concat(),
            gzip(&vec![b'x'; 256 * 1024 + 1]),
            [
                gzip(&vec![b'x'; 128 * 1024]),
                gzip(&vec![b'x'; 128 * 1024 + 1]),
            ]
            .concat(),
        ] {
            let error = decode(invalid, true, 256 * 1024).unwrap_err();
            assert_eq!(error.code, "upstream_unexpected_response");
            assert!(!serde_json::to_string(&error).unwrap().contains("sentinel"));
        }
        assert_eq!(decode(bytes.to_vec(), false, bytes.len()).unwrap(), bytes);
    }

    #[tokio::test]
    async fn encoding_header_and_wire_limits_fail_closed() {
        use axum::{Router, http::header, routing::get};
        for (encoding, body, limit, valid) in [
            ("gzip", gzip(b"{}"), 100, true),
            ("gzip", gzip(&[b'x'; 101]), 100, false),
            ("identity", vec![b'x'; 101], 100, false),
            ("br", vec![b'x'], 100, false),
            ("gzip, gzip", gzip(b"{}"), 100, false),
            ("gzip", b"not-gzip-secret-sentinel".to_vec(), 100, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let task =
                tokio::spawn(async move {
                    axum::serve(listener, Router::new().route("/", get(move || async move {
                    ([(header::CONTENT_ENCODING, encoding)], body)
                }))).await.unwrap();
                });
            let response = reqwest::get(format!("http://{address}/")).await.unwrap();
            let result = bounded(response, limit).await;
            assert_eq!(result.is_ok(), valid);
            task.abort();
        }
    }
}
