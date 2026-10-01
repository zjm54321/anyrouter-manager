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
    pub checkin_state_path: Option<PathBuf>,
    pub frontend_dir: PathBuf,
    pub browser_helper_dir: PathBuf,
    pub browser_helper_executable: Option<PathBuf>,
    pub container_mode: bool,
    pub cookie_secure: bool,
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
            checkin_state_path: None,
            frontend_dir: "frontend/dist".into(),
            browser_helper_dir: "tools/browser-helper".into(),
            browser_helper_executable: None,
            container_mode: false,
            cookie_secure: false,
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
    pub fn checkin_path(&self) -> PathBuf {
        self.checkin_state_path
            .clone()
            .unwrap_or_else(|| self.state_path.with_file_name("checkin.json"))
    }
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let text = std::fs::read_to_string(path).map_err(|_| "Cannot read configuration file.")?;
        let config: Self = toml::from_str(&text).map_err(|_| "Invalid configuration file.")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if resolved_destination(&self.checkin_path())? == resolved_destination(&self.state_path)? {
            return Err("Check-in state must be separate from account state.");
        }
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
        if (!self.bind.ip().is_loopback() && !self.container_mode)
            || self.helper_timeout == 0
            || self.helper_timeout > 120
        {
            return Err(
                "Non-loopback bind requires container_mode; helper_timeout must be 1..120 seconds.",
            );
        }
        if self
            .browser_helper_executable
            .as_ref()
            .is_some_and(|p| !p.is_absolute())
        {
            return Err("browser_helper_executable must be an absolute operator-controlled path.");
        }
        if self.allowed_origin.is_empty() {
            return Err("Configure at least one allowed_origin.");
        }
        for origin in &self.allowed_origin {
            let url = reqwest::Url::parse(origin).map_err(|_| "Invalid allowed_origin.")?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none_or(|host| host.contains('*'))
                || url.origin().ascii_serialization() != *origin
            {
                return Err(
                    "allowed_origin must be an exact HTTP(S) origin without credentials, path or wildcards.",
                );
            }
            let local = loopback_host(url.host_str().unwrap_or_default());
            if !local && (!self.container_mode || url.scheme() != "https" || !self.cookie_secure) {
                return Err("Remote origins require container_mode, HTTPS and cookie_secure.");
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
        if self.bind.ip().is_loopback() {
            hosts.insert(self.bind.to_string());
        }
        hosts.insert(format!("127.0.0.1:{}", self.bind.port()));
        hosts.insert(format!("localhost:{}", self.bind.port()));
        hosts
    }
}

// Resolve existing components before interpreting `..`: a symlink's parent is
// the target's parent, not the lexical parent of the symlink spelling.
fn resolved_destination(path: &Path) -> Result<PathBuf, &'static str> {
    use std::path::Component;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| "Cannot resolve state paths.")?
            .join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => {
                resolved.push(other.as_os_str());
                match std::fs::symlink_metadata(&resolved) {
                    Ok(_) => {
                        resolved = std::fs::canonicalize(&resolved)
                            .map_err(|_| "Cannot resolve state paths.")?
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err("Cannot resolve state paths."),
                }
            }
        }
    }
    Ok(resolved)
}

fn loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
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
    fn state_aliases_rejected_before_files_exist() {
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let destination = dir.path().join("missing/account.json");
        let mut config = Config {
            root_key: "Test-Only-Root-91c7ce8aa-2345-SufficientEntropy".into(),
            state_path: destination.clone(),
            ..Config::default()
        };
        config.checkin_state_path = Some(destination.strip_prefix(&cwd).unwrap().into());
        assert!(config.validate().is_err());
        config.checkin_state_path = Some(dir.path().join("missing/child/../account.json"));
        assert!(config.validate().is_err());
        std::os::unix::fs::symlink(dir.path(), dir.path().join("alias")).unwrap();
        config.checkin_state_path = Some(dir.path().join("alias/missing/account.json"));
        assert!(config.validate().is_err());
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("deep")).unwrap();
        config.checkin_state_path = Some(dir.path().join("deep/../missing/account.json"));
        assert!(config.validate().is_err());
        config.checkin_state_path = Some(dir.path().join("missing/checkin.json"));
        config.validate().unwrap();
        assert!(!destination.exists());
    }
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

    #[test]
    fn container_origins_and_executable_are_explicit() {
        let mut config: Config =
            toml::from_str("root_key = 'Test-Only-Root-91c7e8aa-2345-SufficientEntropy'").unwrap();
        config.bind = "0.0.0.0:8080".parse().unwrap();
        assert!(config.validate().is_err());
        config.container_mode = true;
        config.validate().unwrap();
        assert!(!config.allowed_hosts().contains("0.0.0.0:8080"));
        assert!(config.allowed_hosts().contains("127.0.0.1:8080"));
        config.allowed_origin = vec!["https://manager.example".into()];
        assert!(config.validate().is_err());
        config.cookie_secure = true;
        config.validate().unwrap();
        assert!(config.allowed_hosts().contains("manager.example"));
        for origin in [
            "http://manager.example",
            "https://manager.example/",
            "https://manager.example?x=1",
            "https://manager.example#x",
            "https://fake:fake@manager.example",
            "https://*.example",
        ] {
            config.allowed_origin = vec![origin.into()];
            assert!(config.validate().is_err(), "{origin}");
        }
        config.allowed_origin = vec!["https://manager.example".into()];
        config.browser_helper_executable = Some("relative/python".into());
        assert!(config.validate().is_err());
        config.browser_helper_executable = Some("/app/venv/bin/python".into());
        config.validate().unwrap();
    }
}
