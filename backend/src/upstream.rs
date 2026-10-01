use crate::{
    error::SafeError,
    model::{Balance, Credentials, ListedKey, decimal, full_key, now, positive_id},
};
use axum::http::{HeaderMap, header};
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;

pub const UPSTREAM: &str = "https://anyrouter.top";
pub const USER_AGENT: &str = "AnyRouter-Manager/0.1";

pub struct Upstream {
    pub(crate) base: String,
    pub(crate) client: reqwest::Client,
}

impl Upstream {
    pub fn production() -> Result<Self, reqwest::Error> {
        Self::new(UPSTREAM.to_owned())
    }
    fn new(base: String) -> Result<Self, reqwest::Error> {
        Ok(Self {
            base,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .user_agent(USER_AGENT)
                .build()?,
        })
    }
    #[cfg(test)]
    pub(crate) fn mock(base: String) -> Self {
        Self::new(base).expect("mock client")
    }

    async fn json(
        &self,
        credentials: &Credentials,
        method: reqwest::Method,
        path: &str,
    ) -> Result<Value, SafeError> {
        // Test transport replaces only the network destination; cookie policy always
        // evaluates against the actual production HTTPS origin and endpoint path.
        let headers = credential_headers(credentials, path)?;
        let response = self
            .client
            .request(method, format!("{}{path}", self.base))
            .headers(headers)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| SafeError::new("upstream_unavailable"))?;
        let status = response.status();
        if status.is_server_error() {
            return Err(SafeError::new("upstream_unavailable"));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| SafeError::new("upstream_unavailable"))?;
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(unexpected());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
                let challenge = (text.contains("<html")
                    || text.contains("<!doctype html")
                    || text.contains("<script"))
                    && [
                        "cf-chl-",
                        "challenge-platform",
                        "just a moment",
                        "checking your browser",
                        "waf challenge",
                    ]
                    .iter()
                    .any(|s| text.contains(s));
                return Err(SafeError::new(if challenge {
                    "upstream_challenge"
                } else {
                    "upstream_unexpected_response"
                }));
            }
        };
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(SafeError::new("upstream_session_expired"));
        }
        if !status.is_success() || value.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(unexpected());
        }
        value.get("data").cloned().ok_or_else(unexpected)
    }

    pub async fn balance(&self, credentials: &Credentials) -> Result<Balance, SafeError> {
        let data = self
            .json(credentials, reqwest::Method::GET, "/api/user/self")
            .await?;
        if identifier(&data["id"])? != credentials.api_user {
            return Err(unexpected());
        }
        let quota_raw = raw_decimal(&data["quota"])?;
        let used_quota_raw = raw_decimal(&data["used_quota"])?;
        Ok(Balance {
            quota_raw,
            used_quota_raw,
            fetched_at: now(),
        })
    }

    pub async fn keys(&self, credentials: &Credentials) -> Result<Vec<ListedKey>, SafeError> {
        let mut keys = Vec::new();
        let mut total_expected = None;
        for page in 0..20 {
            let data = self
                .json(
                    credentials,
                    reqwest::Method::GET,
                    &format!("/api/token/?p={page}&size=100"),
                )
                .await?;
            let items = if let Some(items) = data.as_array() {
                items
            } else {
                data.get("items")
                    .and_then(Value::as_array)
                    .ok_or_else(unexpected)?
            };
            if items.len() > 100 {
                return Err(unexpected());
            }
            if let Some(total) = data.get("total") {
                let count = raw_decimal(total)?
                    .parse::<usize>()
                    .map_err(|_| unexpected())?;
                if count > 2000 {
                    return Err(SafeError::new("too_many_keys"));
                }
                if total_expected.is_some_and(|old| old != count) {
                    return Err(unexpected());
                }
                total_expected = Some(count);
            }
            for item in items {
                let id = identifier(&item["id"])?;
                if keys.iter().any(|key: &ListedKey| key.id == id) {
                    return Err(unexpected());
                }
                let name = item["name"]
                    .as_str()
                    .filter(|s| s.len() <= 512)
                    .ok_or_else(unexpected)?
                    .to_owned();
                let status = raw_decimal(&item["status"])?;
                let raw = match item.get("key") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(key)) => full_key(key).then(|| key.clone()),
                    _ => return Err(unexpected()),
                };
                keys.push(ListedKey {
                    id,
                    name,
                    raw,
                    enabled: status == "1",
                });
            }
            if let Some(total) = total_expected {
                if keys.len() > total || (items.len() < 100 && keys.len() != total) {
                    return Err(unexpected());
                }
                if keys.len() == total {
                    return Ok(keys);
                }
            } else if items.len() < 100 {
                return Ok(keys);
            }
        }
        Err(SafeError::new("too_many_keys"))
    }

    pub async fn key(
        &self,
        credentials: &Credentials,
        key: &ListedKey,
    ) -> Result<String, SafeError> {
        if !key.enabled {
            return Err(SafeError::new("key_material_unavailable"));
        }
        if let Some(raw) = &key.raw {
            return Ok(raw.clone());
        }
        let result = self
            .json(
                credentials,
                reqwest::Method::POST,
                &format!("/api/token/{}/key", key.id),
            )
            .await;
        match result {
            Ok(data) => data
                .get("key")
                .and_then(Value::as_str)
                .filter(|key| full_key(key))
                .map(str::to_owned)
                .ok_or_else(|| SafeError::new("key_material_unavailable")),
            Err(error) if error.code == "upstream_unexpected_response" => {
                Err(SafeError::new("key_material_unavailable"))
            }
            Err(error) => Err(error),
        }
    }
}

fn credential_headers(credentials: &Credentials, path: &str) -> Result<HeaderMap, SafeError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        credentials
            .cookie_header_for(&format!("{UPSTREAM}{path}"))
            .ok_or_else(|| SafeError::new("upstream_session_expired"))?,
    );
    headers.insert(
        "New-Api-User",
        credentials.api_user.parse().map_err(|_| unexpected())?,
    );
    Ok(headers)
}

fn unexpected() -> SafeError {
    SafeError::new("upstream_unexpected_response")
}
fn raw_decimal(value: &Value) -> Result<String, SafeError> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(unexpected()),
    };
    if decimal(&text) {
        Ok(text)
    } else {
        Err(unexpected())
    }
}
fn identifier(value: &Value) -> Result<String, SafeError> {
    let text = raw_decimal(value)?;
    if positive_id(&text) {
        Ok(text)
    } else {
        Err(unexpected())
    }
}
