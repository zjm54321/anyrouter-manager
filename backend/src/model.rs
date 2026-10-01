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

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeySummary {
    pub id: String,
    pub name: String,
    pub masked: String,
    pub enabled: bool,
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
            enabled: self.enabled,
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
            "keys": self.keys.iter().map(ListedKey::summary).collect::<Vec<_>>(),
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
    pub account_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedKey {
    pub id: String,
    pub name: String,
    pub key: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub revision: String,
    pub username: String,
    pub upstream_user_id: String,
    pub balance: Balance,
    pub credentials: Credentials,
    pub keys: Vec<KeySummary>,
    pub selected_key: Option<SelectedKey>,
    pub added_at: String,
}

impl Entry {
    pub fn from_active(a: Active) -> Self {
        Self {
            id: id(),
            revision: a.revision,
            username: a.username,
            upstream_user_id: a.upstream_user_id,
            balance: a.balance,
            credentials: a.credentials,
            keys: vec![KeySummary {
                id: a.key_id.clone(),
                name: a.key_name.clone(),
                masked: "••••••••".into(),
                enabled: true,
            }],
            selected_key: Some(SelectedKey {
                id: a.key_id,
                name: a.key_name,
                key: a.key,
            }),
            added_at: a.activated_at,
        }
    }
    pub fn validate(&self) -> bool {
        let mut ids = std::collections::HashSet::new();
        uuid::Uuid::parse_str(&self.id).is_ok()
            && !self.revision.is_empty()
            && self.revision.len() <= 128
            && !self.revision.chars().any(char::is_control)
            && positive_id(&self.upstream_user_id)
            && self.upstream_user_id == self.credentials.api_user
            && self.credentials.validate_structure()
            && !self.username.is_empty()
            && self.username.len() <= 512
            && !self.username.chars().any(char::is_control)
            && DateTime::parse_from_rfc3339(&self.added_at).is_ok()
            && DateTime::parse_from_rfc3339(&self.balance.fetched_at).is_ok()
            && decimal(&self.balance.quota_raw)
            && decimal(&self.balance.used_quota_raw)
            && self.keys.len() <= 2000
            && self.keys.iter().all(|k| {
                positive_id(&k.id)
                    && k.name.len() <= 512
                    && k.masked == "••••••••"
                    && ids.insert(&k.id)
            })
            && self
                .selected_key
                .as_ref()
                .is_none_or(|k| positive_id(&k.id) && k.name.len() <= 512 && full_key(&k.key))
    }
    pub fn snapshot(&self) -> Option<Active> {
        let key = self.selected_key.as_ref()?;
        Some(Active {
            revision: self.revision.clone(),
            username: self.username.clone(),
            upstream_user_id: self.upstream_user_id.clone(),
            balance: self.balance.clone(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            key: key.key.clone(),
            credentials: self.credentials.clone(),
            activated_at: self.added_at.clone(),
        })
    }
    pub fn dto(&self) -> serde_json::Value {
        serde_json::json!({"id":self.id,"revision":self.revision,"username":self.username,"upstream_user_id":self.upstream_user_id,"balance":self.balance,"keys":self.keys,"selected_key":self.selected_key.as_ref().map(|k| serde_json::json!({"id":k.id,"name":k.name,"masked":"••••••••","enabled":self.keys.iter().find(|m| m.id==k.id).is_some_and(|m|m.enabled)})),"added_at":self.added_at})
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Portfolio {
    pub version: u32,
    pub accounts: Vec<Entry>,
    pub route_account_id: Option<String>,
}
impl Default for Portfolio {
    fn default() -> Self {
        Self {
            version: 2,
            accounts: Vec::new(),
            route_account_id: None,
        }
    }
}
impl Portfolio {
    pub fn from_active(active: Active) -> Self {
        let entry = Entry::from_active(active);
        Self {
            version: 2,
            route_account_id: Some(entry.id.clone()),
            accounts: vec![entry],
        }
    }
    pub fn validate(&self) -> bool {
        let mut ids = std::collections::HashSet::new();
        let mut users = std::collections::HashSet::new();
        self.version == 2
            && self.accounts.len() <= 64
            && self
                .accounts
                .iter()
                .all(|a| a.validate() && ids.insert(&a.id) && users.insert(&a.upstream_user_id))
            && self.route_account_id.as_ref().is_none_or(|id| {
                self.accounts
                    .iter()
                    .any(|a| &a.id == id && a.selected_key.is_some())
            })
    }
    pub fn route(&self) -> Option<&Entry> {
        self.route_account_id
            .as_ref()
            .and_then(|id| self.accounts.iter().find(|a| &a.id == id))
    }
}
