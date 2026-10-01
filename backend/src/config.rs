use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use subtle::ConstantTimeEq;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub root_key: String,
    pub bind: SocketAddr,
    pub state_path: PathBuf,
    pub frontend_dir: PathBuf,
    pub browser_helper_dir: PathBuf,
    pub helper_timeout: u64,
    pub login_diagnostics: bool,
    pub allowed_origin: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root_key: String::new(),
            bind: "127.0.0.1:8080".parse().expect("literal socket address"),
            state_path: "data/account.json".into(),
            frontend_dir: "frontend/dist".into(),
            browser_helper_dir: "tools/browser-helper".into(),
            helper_timeout: 90,
            login_diagnostics: false,
            allowed_origin: [
                "http://localhost:5173",
                "http://127.0.0.1:5173",
                "http://localhost:8080",
                "http://127.0.0.1:8080",
            ]
            .map(str::to_owned)
            .into(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let text = std::fs::read_to_string(path).map_err(|_| "Cannot read configuration file.")?;
        let config: Self = toml::from_str(&text).map_err(|_| "Invalid configuration file.")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let lower = self.root_key.to_ascii_lowercase();
        if self.root_key.len() < 32
            || !self.root_key.bytes().all(|b| b.is_ascii_graphic())
            || ["replace", "placeholder", "change_me", "changeme", "example"]
                .iter()
                .any(|s| lower.contains(s))
            || self.root_key.bytes().collect::<HashSet<_>>().len() < 8
        {
            return Err("Set a non-placeholder high-entropy root_key of at least 32 bytes.");
        }
        if !self.bind.ip().is_loopback() || self.helper_timeout == 0 || self.helper_timeout > 120 {
            return Err("Use a loopback bind and helper_timeout between 1 and 120 seconds.");
        }
        if self.allowed_origin.is_empty() {
            return Err("Configure at least one allowed_origin.");
        }
        for origin in &self.allowed_origin {
            let url = reqwest::Url::parse(origin).map_err(|_| "Invalid allowed_origin.")?;
            if !matches!(url.scheme(), "http" | "https")
                || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
                || url.origin().ascii_serialization() != *origin
            {
                return Err("allowed_origin must be an exact localhost/loopback HTTP origin.");
            }
        }
        Ok(())
    }

    pub fn allowed_hosts(&self) -> HashSet<String> {
        let mut hosts: HashSet<String> = self
            .allowed_origin
            .iter()
            .filter_map(|origin| {
                let url = reqwest::Url::parse(origin).ok()?;
                Some(match url.port() {
                    Some(port) => format!("{}:{port}", url.host_str()?),
                    None => url.host_str()?.to_owned(),
                })
            })
            .collect();
        hosts.insert(self.bind.to_string());
        hosts.insert(format!("localhost:{}", self.bind.port()));
        hosts
    }
}

pub struct RootAuth([u8; 32]);

impl RootAuth {
    pub fn new(key: &str) -> Self {
        Self(Sha256::digest(key.as_bytes()).into())
    }
    pub fn matches(&self, key: &str) -> bool {
        let hash: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        bool::from(self.0.ct_eq(&hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_diagnostics_is_explicit_and_backward_compatible() {
        let old: Config =
            toml::from_str("root_key = 'Test-Only-Root-91c7e8aa-2345-SufficientEntropy'").unwrap();
        old.validate().unwrap();
        assert!(!old.login_diagnostics);
        let enabled: Config = toml::from_str(
            "root_key = 'Test-Only-Root-91c7e8aa-2345-SufficientEntropy'\nlogin_diagnostics = true",
        )
        .unwrap();
        enabled.validate().unwrap();
        assert!(enabled.login_diagnostics);
        assert!(toml::from_str::<Config>("login_diagnostics = 'true'").is_err());
        assert!(Config::default().validate().is_err());
    }
}
