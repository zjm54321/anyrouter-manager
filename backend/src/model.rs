use crate::error::SafeError;
use axum::http::HeaderValue;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub fn now() -> String {
    Utc::now().to_rfc3339()
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Balance {
    pub quota_raw: String,
    pub used_quota_raw: String,
    pub fetched_at: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub cookies: Vec<Cookie>,
    pub api_user: String,
}

impl Credentials {
    pub fn validate(&self) -> bool {
        self.validate_structure()
            && self
                .cookie_header_for(&format!("{}/api/user/self", crate::upstream::UPSTREAM))
                .is_some()
    }

    pub fn validate_structure(&self) -> bool {
        positive_id(&self.api_user)
            && !self.cookies.is_empty()
            && self.cookies.len() <= 100
            && self.cookies.iter().all(|c| {
                matches!(c.domain.as_str(), "anyrouter.top" | ".anyrouter.top")
                    && !c.name.is_empty()
                    && c.name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                    && !c.value.is_empty()
                    && c.value.len() <= 8192
                    && c.value
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && !b"\";,\\".contains(&b))
                    && c.path.starts_with('/')
                    && c.path.len() <= 2048
                    && !c.path.chars().any(char::is_control)
                    && c.expires.is_none_or(|e| e.is_finite() && e >= -1.0)
            })
    }

    pub fn cookie_header_for(&self, target: &str) -> Option<HeaderValue> {
        let url = reqwest::Url::parse(target).ok()?;
        let host = url.host_str()?;
        let now = Utc::now().timestamp() as f64;
        let mut selected = self
            .cookies
            .iter()
            .filter(|c| {
                let domain = c.domain.strip_prefix('.').unwrap_or(&c.domain);
                let domain_match = host == domain
                    || (c.domain.starts_with('.') && host.ends_with(&format!(".{domain}")));
                let path_match = url.path() == c.path
                    || (url.path().starts_with(&c.path)
                        && (c.path.ends_with('/')
                            || url.path().as_bytes().get(c.path.len()) == Some(&b'/')));
                (c.expires.is_none_or(|e| e < 0.0 || e > now))
                    && (!c.secure || url.scheme() == "https")
                    && domain_match
                    && path_match
            })
            .collect::<Vec<_>>();
        // Stable sort retains helper/browser creation order for equal-length paths.
        selected.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        let joined = selected
            .into_iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        if joined.is_empty() {
            None
        } else {
            HeaderValue::from_str(&joined).ok()
        }
    }
}

pub fn positive_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| n > 0)
}

pub fn full_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16384
        && value.bytes().all(|b| b.is_ascii_graphic())
        && !value.contains(['*', '•', '…'])
        && !value.contains("...")
        && ![
            "masked",
            "[masked]",
            "<masked>",
            "redacted",
            "[redacted]",
            "hidden",
            "null",
            "none",
        ]
        .contains(&value.to_ascii_lowercase().as_str())
        && HeaderValue::from_str(&format!("Bearer {value}")).is_ok()
}

#[derive(Clone, Serialize)]
pub struct KeySummary {
    pub id: String,
    pub name: String,
    pub masked: String,
}

#[derive(Clone)]
pub struct ListedKey {
    pub id: String,
    pub name: String,
    pub raw: Option<String>,
    pub enabled: bool,
}
impl ListedKey {
    pub fn summary(&self) -> KeySummary {
        KeySummary {
            id: self.id.clone(),
            name: self.name.clone(),
            masked: "••••••••".into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Active {
    pub revision: String,
    pub username: String,
    pub upstream_user_id: String,
    pub balance: Balance,
    pub key_id: String,
    pub key_name: String,
    pub key: String,
    pub credentials: Credentials,
    pub activated_at: String,
}

impl Active {
    pub fn validate(&self) -> bool {
        positive_id(&self.revision)
            && positive_id(&self.upstream_user_id)
            && self.upstream_user_id == self.credentials.api_user
            // Persisted cookies may have expired since activation. Their structure
            // must be safe, but expiration must not disable an independent API key.
            && self.credentials.validate_structure()
            && positive_id(&self.key_id)
            && full_key(&self.key)
            && !self.username.is_empty()
            && self.username.len() <= 512
            && self.key_name.len() <= 512
            && DateTime::parse_from_rfc3339(&self.activated_at).is_ok()
            && DateTime::parse_from_rfc3339(&self.balance.fetched_at).is_ok()
            && decimal(&self.balance.quota_raw)
            && decimal(&self.balance.used_quota_raw)
    }
    pub fn dto(&self) -> serde_json::Value {
        serde_json::json!({"revision": self.revision, "username": self.username,
            "upstream_user_id": self.upstream_user_id, "balance": self.balance,
            "selected_key": {"id": self.key_id, "name": self.key_name, "masked": "••••••••"},
            "activated_at": self.activated_at})
    }
}

pub fn decimal(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Clone)]
pub struct Candidate {
    pub id: String,
    pub username: String,
    pub credentials: Credentials,
    pub balance: Balance,
    pub keys: Vec<ListedKey>,
    pub deadline: Instant,
    pub expires_at: String,
}

impl Candidate {
    pub fn dto(&self) -> serde_json::Value {
        serde_json::json!({"id": self.id, "username": self.username,
            "upstream_user_id": self.credentials.api_user, "balance": self.balance,
            "keys": self.keys.iter().filter(|k| k.enabled).map(ListedKey::summary).collect::<Vec<_>>(),
            "expires_at": self.expires_at})
    }
}

#[derive(Clone, Serialize)]
pub struct Operation {
    pub id: String,
    pub kind: &'static str,
    pub status: &'static str,
    pub phase: &'static str,
    pub error: Option<SafeError>,
}
