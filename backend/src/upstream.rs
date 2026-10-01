use crate::system_log::{EventReason, EventStage};
use crate::{
    error::SafeError,
    model::{Balance, Credentials, ListedKey, decimal, full_key, now, positive_id},
};
use axum::http::{HeaderMap, header};
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
                .retry(reqwest::retry::never())
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
    ) -> Result<(Value, u16), SafeError> {
        let stage = if path == "/api/user/self" {
            EventStage::SelfAccount
        } else if method == reqwest::Method::GET {
            EventStage::Tokens
        } else {
            EventStage::Key
        };
        crate::logging::emit(
            crate::log_settings::LogLevel::Debug,
            crate::system_log::EventKind::StorageRead,
            stage,
            None,
            None,
        );
        // Test transport replaces only the network destination; cookie policy always
        // evaluates against the actual production HTTPS origin and endpoint path.
        let headers = credential_headers(credentials, path).map_err(|e| e.at(stage, None))?;
        let response = self
            .client
            .request(method, format!("{}{path}", self.base))
            .headers(headers)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| {
                SafeError::new(if e.is_timeout() {
                    "upstream_timeout"
                } else {
                    "upstream_unavailable"
                })
                .at(stage, None)
            })?;
        let status = response.status();
        crate::logging::emit(
            crate::log_settings::LogLevel::Debug,
            crate::system_log::EventKind::StorageRead,
            stage,
            None,
            Some(status.as_u16()),
        );
        let (_, value) = crate::upstream_body::json(response)
            .await
            .map_err(|e| e.at(stage, Some(status.as_u16())))?;
        crate::upstream_body::data(status, value)
            .map(|value| (value, status.as_u16()))
            .map_err(|e| e.at(stage, Some(status.as_u16())))
    }

    #[cfg(test)]
    pub async fn balance(&self, credentials: &Credentials) -> Result<Balance, SafeError> {
        self.profile(credentials).await.map(|(balance, _)| balance)
    }

    pub async fn profile(
        &self,
        credentials: &Credentials,
    ) -> Result<(Balance, Option<String>), SafeError> {
        let (data, status) = self
            .json(credentials, reqwest::Method::GET, "/api/user/self")
            .await?;
        profile_data(&data, &credentials.api_user)
            .map_err(|e| e.at(EventStage::SelfAccount, Some(status)))
    }

    pub async fn keys(&self, credentials: &Credentials) -> Result<Vec<ListedKey>, SafeError> {
        self.keys_inner(credentials).await.map_err(|mut error| {
            error.stage.get_or_insert(EventStage::Tokens);
            if error.reason.is_none() && error.code == "upstream_unexpected_response" {
                error.reason = Some(EventReason::SchemaInvalid);
            }
            error
        })
    }

    async fn keys_inner(&self, credentials: &Credentials) -> Result<Vec<ListedKey>, SafeError> {
        let mut keys = Vec::new();
        let mut total_expected = None;
        for page in 0..20 {
            let (data, _) = self
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
            Ok((data, _)) => data
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

pub(crate) fn credential_headers(
    credentials: &Credentials,
    path: &str,
) -> Result<HeaderMap, SafeError> {
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
pub(crate) fn identifier(value: &Value) -> Result<String, SafeError> {
    let text = raw_decimal(value)?;
    if positive_id(&text) {
        Ok(text)
    } else {
        Err(unexpected())
    }
}

pub(crate) fn profile_data(
    data: &Value,
    user: &str,
) -> Result<(Balance, Option<String>), SafeError> {
    if !identifier(&data["id"]).is_ok_and(|id| id == user) {
        return Err(SafeError::new("upstream_session_unverified")
            .with_reason(EventReason::IdentityMismatch));
    }
    let username = match data.get("username") {
        None | Some(Value::Null) => None,
        Some(Value::String(s))
            if !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control) =>
        {
            Some(s.clone())
        }
        _ => return Err(unexpected().with_reason(EventReason::SchemaInvalid)),
    };
    Ok((
        Balance {
            quota_raw: raw_decimal(&data["quota"])
                .map_err(|e| e.with_reason(EventReason::QuotaInvalid))?,
            used_quota_raw: raw_decimal(&data["used_quota"])
                .map_err(|e| e.with_reason(EventReason::QuotaInvalid))?,
            fetched_at: now(),
        },
        username,
    ))
}
