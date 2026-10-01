use crate::{
    diagnostics::LoginDiagnostics,
    error::{ErrorSource, SafeError},
    model::{Balance, Cookie, Credentials},
};
use async_trait::async_trait;
use serde::Deserialize;
use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::{
    io::unix::AsyncFd,
    sync::{OwnedSemaphorePermit, Semaphore},
};

static BROWSER_GATE: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));

pub struct LoginOutcome {
    pub credentials: Credentials,
    pub profile: Option<(Balance, Option<String>)>,
}

#[async_trait]
pub trait LoginProvider: Send + Sync {
    async fn verify_session(
        &self,
        _credentials: Credentials,
        _budget: Duration,
    ) -> Result<Credentials, SafeError> {
        Err(SafeError::new("upstream_session_unverified"))
    }
    async fn login(&self, username: String, password: String) -> Result<Credentials, SafeError>;
    async fn login_context(
        &self,
        username: String,
        password: String,
    ) -> Result<LoginOutcome, SafeError> {
        self.login(username, password)
            .await
            .map(|credentials| LoginOutcome {
                credentials,
                profile: None,
            })
    }
    async fn login_with_budget(
        &self,
        username: String,
        password: String,
        budget: Duration,
    ) -> Result<Credentials, SafeError> {
        tokio::time::timeout(budget, self.login(username, password))
            .await
            .map_err(|_| SafeError::new("upstream_timeout"))?
    }
}

pub struct BrowserLogin {
    pub directory: PathBuf,
    pub timeout: Duration,
    pub diagnostics: bool,
    pub executable: Option<PathBuf>,
    pub container_mode: bool,
}

#[derive(Deserialize)]
struct HelperResult {
    ok: bool,
    cookies: Option<Vec<Cookie>>,
    api_user: Option<String>,
    error: Option<String>,
    diagnostics: Option<serde_json::Value>,
}

// Own a std Child rather than a Tokio Child: no Tokio orphan reaper can reap
// the leader behind this guard. Its unreaped identity pins the process-group ID.
struct ProcessGroup {
    child: Child,
    cleaned: bool,
    // Released only after wait confirms the namespace has exited, including cancellation.
    _permit: Option<OwnedSemaphorePermit>,
}
impl ProcessGroup {
    fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        // SAFETY: this direct child has never been reaped. Kill the group BEFORE
        // wait(), including when WNOWAIT observed a zombie leader on normal exit.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        // Transfer only the reap to a blocking worker, so cancellation cleanup never
        // blocks the async executor. No kill using this PGID occurs after wait.
        self.cleaned = true;
    }

    async fn exited_successfully(&self) -> std::io::Result<bool> {
        loop {
            // SAFETY: zeroed siginfo_t is a valid output buffer; WNOWAIT explicitly
            // leaves the exited child waitable and keeps its PID reserved.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { info.si_pid() } != 0 {
                return Ok(info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.cleanup();
    }
}

struct ManagedChild(Option<ProcessGroup>);
enum Lifecycle {
    Namespace(ManagedChild),
    Container(crate::supervisor::Managed),
}
impl Lifecycle {
    async fn exited_successfully(&mut self) -> std::io::Result<bool> {
        match self {
            Self::Namespace(child) => {
                child
                    .0
                    .as_ref()
                    .expect("owned namespace")
                    .exited_successfully()
                    .await
            }
            Self::Container(child) => child.exited_successfully().await,
        }
    }
    async fn close(&mut self) {
        match self {
            Self::Namespace(child) => child.close().await,
            Self::Container(child) => child.close().await,
        }
    }
}
impl ManagedChild {
    async fn close(&mut self) {
        if let Some(mut group) = self.0.take() {
            group.cleanup();
            let _ = tokio::task::spawn_blocking(move || {
                let _ = group.child.wait();
                drop(group); // Keep the browser permit in the reap owner through wait.
            })
            .await;
        }
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Some(mut group) = self.0.take() {
            group.cleanup();
            // std Child has no implicit reap. Exactly one owner performs wait,
            // after the once-only group kill, even when the task was aborted.
            tokio::task::spawn_blocking(move || {
                let _ = group.child.wait();
                drop(group);
            });
        }
    }
}

fn pipe(fd: OwnedFd) -> std::io::Result<AsyncFd<std::fs::File>> {
    let file = std::fs::File::from(fd);
    // SAFETY: fcntl operates on an owned valid pipe descriptor.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    AsyncFd::new(file)
}

async fn write_input(pipe: &AsyncFd<std::fs::File>, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let mut ready = pipe.writable().await?;
        if let Ok(result) = ready.try_io(|fd| fd.get_ref().write(bytes)) {
            let count = result?;
            if count == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[count..];
        }
    }
    Ok(())
}

async fn read_output(pipe: &AsyncFd<std::fs::File>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 8192];
        let mut ready = pipe.readable().await?;
        if let Ok(result) = ready.try_io(|fd| fd.get_ref().read(&mut chunk)) {
            let count = result?;
            if count == 0 {
                return Ok(bytes);
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.len() > 256 * 1024 {
                return Ok(bytes);
            }
        }
    }
}

#[async_trait]
impl LoginProvider for BrowserLogin {
    async fn verify_session(
        &self,
        credentials: Credentials,
        budget: Duration,
    ) -> Result<Credentials, SafeError> {
        let input = serde_json::json!({"mode":"session_verify", "cookies":credentials.cookies, "api_user":credentials.api_user, "timeout_ms":budget.min(self.timeout).as_millis()});
        let result = tokio::time::timeout(budget, self.execute_input(self.command(), input))
            .await
            .map_err(|_| SafeError::new("upstream_timeout"))??;
        if result.api_user != credentials.api_user {
            return Err(SafeError::new("upstream_session_unverified"));
        }
        Ok(result)
    }
    async fn login(&self, username: String, password: String) -> Result<Credentials, SafeError> {
        self.execute(self.command(), username, password).await
    }
    async fn login_with_budget(
        &self,
        username: String,
        password: String,
        budget: Duration,
    ) -> Result<Credentials, SafeError> {
        if budget < Duration::from_secs(1) {
            return Err(SafeError::new("upstream_timeout"));
        }
        let bounded = BrowserLogin {
            directory: self.directory.clone(),
            timeout: budget.min(self.timeout),
            diagnostics: self.diagnostics,
            executable: self.executable.clone(),
            container_mode: self.container_mode,
        };
        tokio::time::timeout(budget, bounded.login(username, password))
            .await
            .map_err(|_| SafeError::new("upstream_timeout"))?
    }
}

impl BrowserLogin {
    fn command(&self) -> Command {
        if self.container_mode {
            let mut command = if let Some(executable) = &self.executable {
                let mut command = Command::new(executable);
                command.args(["-B", "-m", "browser_helper"]);
                command
            } else {
                let mut command = Command::new("uv");
                command.args([
                    "run",
                    "--offline",
                    "--frozen",
                    "--no-sync",
                    "python",
                    "-B",
                    "-m",
                    "browser_helper",
                ]);
                command
            };
            command.current_dir(&self.directory);
            return command;
        }
        let mut command = namespace_command();
        if let Some(executable) = &self.executable {
            command.arg(executable).args(["-B", "-m", "browser_helper"]);
        } else {
            command.args([
                "uv",
                "run",
                "--offline",
                "--frozen",
                "--no-sync",
                "python",
                "-B",
                "-m",
                "browser_helper",
            ]);
        }
        command.current_dir(&self.directory);
        command
    }
}

fn namespace_command() -> Command {
    let mut command = Command::new("unshare");
    command.args([
        "--user",
        "--map-current-user",
        "--pid",
        "--fork",
        "--kill-child=KILL",
        "--mount-proc",
        "--",
    ]);
    command
}

fn parent_death_signal(command: &mut Command) {
    // Capture the backend PID BEFORE fork. The child closure uses only
    // async-signal-safe libc operations (no allocation, formatting, or logging).
    let parent = unsafe { libc::getpid() };
    // SAFETY: prctl/getppid are called before exec with fixed primitive values.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(127);
            }
            Ok(())
        });
    }
}

impl BrowserLogin {
    async fn execute(
        &self,
        command: Command,
        username: String,
        password: String,
    ) -> Result<Credentials, SafeError> {
        self.execute_input(command, serde_json::json!({"username":username,"password":password,"timeout_ms":self.timeout.as_millis()})).await
    }

    async fn execute_input(
        &self,
        mut command: Command,
        input: serde_json::Value,
    ) -> Result<Credentials, SafeError> {
        let permit = BROWSER_GATE
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| SafeError::new("browser_unavailable"))?;
        // The config is the sole opt-in, including if the backend inherited this
        // variable from its caller. Apply at execution to cover every command.
        if self.diagnostics {
            command.env("BROWSER_HELPER_DIAGNOSTICS", "1");
        } else {
            command.env_remove("BROWSER_HELPER_DIAGNOSTICS");
        }
        let (mut managed, stdin, stdout) = if self.container_mode {
            let exe = std::env::current_exe().map_err(|_| SafeError::new("browser_unavailable"))?;
            let (managed, stdin, stdout) = crate::supervisor::spawn(&exe, &command, Some(permit))
                .map_err(|_| SafeError::new("browser_unavailable"))?;
            (Lifecycle::Container(managed), stdin, stdout)
        } else {
            parent_death_signal(&mut command);
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(|_| {
                    SafeError::new("upstream_unavailable").with_source(ErrorSource::Helper)
                })?;
            let stdin = child.stdin.take().expect("piped input");
            let stdout = child.stdout.take().expect("piped output");
            let managed = ManagedChild(Some(ProcessGroup {
                child,
                cleaned: false,
                _permit: Some(permit),
            }));
            (Lifecycle::Namespace(managed), stdin, stdout)
        };
        let result = tokio::time::timeout(self.timeout, async {
            let input = serde_json::to_vec(&input)
                .map_err(|_| SafeError::new("upstream_unexpected_response"))?;
            if input.len() > 32 * 1024 {
                return Err(SafeError::new("helper_input_invalid"));
            }
            let stdin = pipe(stdin.into()).map_err(|_| SafeError::new("upstream_unavailable"))?;
            write_input(&stdin, &input)
                .await
                .map_err(|_| SafeError::new("upstream_unavailable"))?;
            write_input(&stdin, b"\n")
                .await
                .map_err(|_| SafeError::new("upstream_unavailable"))?;
            drop(stdin);
            drop(input);
            let stdout = pipe(stdout.into()).map_err(|_| SafeError::new("upstream_unavailable"))?;
            let bytes = read_output(&stdout)
                .await
                .map_err(|_| SafeError::new("upstream_unavailable"))?;
            if bytes.len() > 256 * 1024 {
                return Err(SafeError::new("upstream_unexpected_response"));
            }
            let success = managed
                .exited_successfully()
                .await
                .map_err(|_| SafeError::new("upstream_unavailable"))?;
            // The helper deliberately exits nonzero for structured failures. Parse
            // its bounded stdout before classifying the process status, otherwise
            // explicit challenge/credential errors are hidden as unavailable.
            let output: HelperResult = serde_json::from_slice(&bytes).map_err(|_| {
                SafeError::new(if success {
                    "upstream_unexpected_response"
                } else {
                    "upstream_unavailable"
                })
            })?;
            if !output.ok {
                let code = match output.error.as_deref() {
                    Some("invalid_credentials") => "invalid_credentials",
                    Some("session_expired") => "upstream_session_expired",
                    Some("upstream_challenge" | "challenge_or_block") => "upstream_challenge",
                    Some("upstream_unavailable") => "upstream_unavailable",
                    Some("login_form_unavailable") => "login_form_unavailable",
                    Some("user_self_unverified") => "upstream_session_unverified",
                    Some("login_failed") => "upstream_login_failed",
                    Some("invalid_input") => "helper_input_invalid",
                    Some("browser_unavailable") => "browser_unavailable",
                    Some("timeout") => "upstream_timeout",
                    _ => "upstream_unexpected_response",
                };
                let mut error =
                    SafeError::new(code).with_source(if code == "invalid_credentials" {
                        ErrorSource::Credentials
                    } else {
                        ErrorSource::Helper
                    });
                if self.diagnostics {
                    error.diagnostics = output
                        .diagnostics
                        .and_then(|value| serde_json::from_value::<LoginDiagnostics>(value).ok());
                }
                return Err(error);
            }
            if !success {
                return Err(SafeError::new("upstream_unavailable"));
            }
            let credentials = Credentials {
                cookies: output.cookies.unwrap_or_default(),
                api_user: output.api_user.unwrap_or_default(),
            };
            if !credentials.validate() {
                return Err(SafeError::new("upstream_unexpected_response")
                    .with_source(ErrorSource::Credentials));
            }
            Ok(credentials)
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(SafeError::new("upstream_timeout")),
        };
        managed.close().await;
        result.map_err(|mut error| {
            if error.source.is_none() {
                error.source = Some(ErrorSource::Helper);
            }
            error
        })
    }
}

pub async fn wait_browser_idle() {
    let _permit = BROWSER_GATE.clone().acquire_owned().await;
}

pub async fn isolation_available() -> bool {
    let mut command = namespace_command();
    command
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    parent_death_signal(&mut command);
    let Ok(child) = command.spawn() else {
        return false;
    };
    let mut managed = ManagedChild(Some(ProcessGroup {
        child,
        cleaned: false,
        _permit: None,
    }));
    let ok = tokio::time::timeout(
        Duration::from_secs(2),
        managed.0.as_ref().unwrap().exited_successfully(),
    )
    .await
    .is_ok_and(|r| r.unwrap_or(false));
    managed.close().await;
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn browser_permit_survives_cancel_until_reap_before_next_generation() {
        let directory = tempfile::tempdir().unwrap();
        let host_proc = std::fs::File::open("/proc").unwrap();
        let fd = host_proc.as_raw_fd();
        let mut unrelated = Command::new("sleep").arg("30").spawn().unwrap();
        let mut previous_pidfd: Option<OwnedFd> = None;
        for generation in 0..2 {
            let marker = directory.path().join(format!("generation-{generation}"));
            let mut command = namespace_command();
            command.args(["python", "-c", &format!("import os,sys,time;sys.stdin.readline();open(sys.argv[1],'w').write(os.readlink('/proc/self/fd/{fd}/self'));time.sleep(30)")]).arg(&marker);
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let helper = BrowserLogin {
                directory: PathBuf::new(),
                timeout: Duration::from_secs(30),
                diagnostics: false,
                executable: None,
                container_mode: false,
            };
            let task =
                tokio::spawn(
                    async move { helper.execute(command, "fake".into(), "fake".into()).await },
                );
            let pid = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok(s) = std::fs::read_to_string(&marker)
                        && let Ok(p) = s.parse::<i32>()
                    {
                        break p;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            if let Some(previous) = previous_pidfd.take() {
                let mut poll = libc::pollfd {
                    fd: previous.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                assert_eq!(
                    unsafe { libc::poll(&mut poll, 1, 0) },
                    1,
                    "next generation must not spawn before previous namespace exit"
                );
            }
            use std::os::fd::FromRawFd;
            let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
            assert!(raw >= 0);
            previous_pidfd = Some(unsafe { OwnedFd::from_raw_fd(raw) });
            task.abort();
            assert!(task.await.err().unwrap().is_cancelled());
        }
        wait_browser_idle().await;
        let previous = previous_pidfd.unwrap();
        let mut poll = libc::pollfd {
            fd: previous.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 1);
        assert!(
            unrelated.try_wait().unwrap().is_none(),
            "cleanup must not reap/kill adjacent child"
        );
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }

    #[tokio::test]
    async fn sigterm_server_harness_child() {
        let Some(directory) = std::env::var_os("AR_TEST_SIGTERM_DIRECTORY") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let config = crate::config::Config {
            root_key: "Test-Only-Root-91c7e8aa-2345-SufficientEntropy".into(),
            state_path: directory.join("account.json"),
            browser_helper_dir: directory.clone(),
            browser_helper_executable: Some(directory.join("python-helper")),
            ..Default::default()
        };
        let browser = Arc::new(BrowserLogin {
            directory: directory.clone(),
            executable: config.browser_helper_executable.clone(),
            container_mode: false,
            timeout: Duration::from_secs(90),
            diagnostics: false,
        });
        let app = crate::app::App::new(
            config,
            None,
            crate::upstream::Upstream::mock("http://127.0.0.1:1".into()),
            browser,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::fs::write(
            directory.join("address"),
            listener.local_addr().unwrap().to_string(),
        )
        .unwrap();
        crate::serve_runtime(app, listener).await.unwrap();
    }

    #[tokio::test]
    async fn real_sigterm_stops_server_and_reaps_browser_without_state_commit() {
        let directory = tempfile::tempdir().unwrap();
        let python = Command::new("python")
            .args(["-c", "import sys;print(sys.executable)"])
            .output()
            .unwrap();
        let python = String::from_utf8(python.stdout).unwrap();
        let script = format!(
            "#!{}\nimport os,sys,time\nsys.stdin.readline()\nopen('namespace-running','w').write('yes')\nwhile True: time.sleep(1)\n",
            python.trim()
        );
        let executable = directory.path().join("python-helper");
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut server = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "helper::tests::sigterm_server_harness_child"])
            .env("AR_TEST_SIGTERM_DIRECTORY", directory.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let address = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(s) = std::fs::read_to_string(directory.path().join("address")) {
                    break s;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{address}/api/admin/session"))
            .header("host", "127.0.0.1:8080")
            .header("origin", "http://localhost:5173")
            .bearer_auth("Test-Only-Root-91c7e8aa-2345-SufficientEntropy")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let response = client
            .post(format!("http://{address}/api/account/login"))
            .header("host", "127.0.0.1:8080")
            .header("origin", "http://localhost:5173")
            .header("cookie", cookie)
            .json(&serde_json::json!({"username":"fake","password":"fake"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !directory.path().join("namespace-running").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // Pin direct unshare child identity before sending real SIGTERM to our own server.
        let children = std::fs::read_dir(format!("/proc/{}/task", server.id()))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path().join("children")).ok())
            .collect::<Vec<_>>()
            .join(" ");
        let pid = children
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<i32>()
            .unwrap();
        use std::os::fd::FromRawFd;
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
        assert!(raw >= 0);
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
        assert_eq!(unsafe { libc::kill(server.id() as i32, libc::SIGTERM) }, 0);
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let Some(status) = server.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut poll = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 1);
        assert!(!directory.path().join("account.json").exists());
    }

    #[tokio::test]
    async fn diagnostics_opt_in_env_and_nonzero_failure_allowlist() {
        for enabled in [false, true] {
            let helper = BrowserLogin {
                directory: PathBuf::new(),
                timeout: Duration::from_secs(3),
                diagnostics: enabled,
                executable: None,
                container_mode: false,
            };
            let expected = if enabled { "1" } else { "absent" };
            let mut command = namespace_command();
            command.env("BROWSER_HELPER_DIAGNOSTICS", "unexpected-inherited-value");
            let output = serde_json::json!({"ok": false, "error": "login_failed", "diagnostics": crate::diagnostics::fixture(), "password": "sentinel-top-level-password", "query": "sentinel-query"});
            command.args(["python", "-c", &format!("import os,sys;sys.stdin.readline();assert os.environ.get('BROWSER_HELPER_DIAGNOSTICS','absent')=='{expected}';print(sys.argv[1]);sys.exit(1)")]).arg(output.to_string());
            let safe = helper
                .execute(command, "fake".into(), "fake-password".into())
                .await
                .err()
                .unwrap();
            assert_eq!(safe.code, "upstream_login_failed");
            assert_eq!(safe.source, Some(ErrorSource::Helper));
            let dto = serde_json::to_value(&safe).unwrap();
            assert_eq!(
                dto.get("diagnostics"),
                enabled.then_some(&crate::diagnostics::fixture())
            );
            let text = dto.to_string();
            for secret in ["sentinel", "fake-password", "unexpected-inherited-value"] {
                assert!(!text.contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn malformed_diagnostics_drop_entirely_without_changing_error() {
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_secs(3),
            diagnostics: true,
            executable: None,
            container_mode: false,
        };
        for (field, value) in [
            ("login_status", serde_json::json!(600)),
            (
                "phase",
                serde_json::json!("<script>sentinel-password</script>"),
            ),
            ("password", serde_json::json!("sentinel-password")),
            ("query", serde_json::json!("sentinel-query")),
            ("key_count", serde_json::json!(2000)),
        ] {
            let mut diagnostics = crate::diagnostics::fixture();
            diagnostics[field] = value;
            let output = serde_json::json!({"ok": false, "error": "user_self_unverified", "diagnostics": diagnostics});
            let mut command = namespace_command();
            command
                .args([
                    "python",
                    "-c",
                    "import sys;sys.stdin.readline();print(sys.argv[1]);sys.exit(1)",
                ])
                .arg(output.to_string());
            let safe = helper
                .execute(command, "fake".into(), "fake-password".into())
                .await
                .err()
                .unwrap();
            assert_eq!(safe.code, "upstream_session_unverified");
            assert!(safe.diagnostics.is_none());
            let dto = serde_json::to_string(&safe).unwrap();
            assert!(!dto.contains("diagnostics"));
            assert!(!dto.contains("sentinel"));
        }
    }

    #[tokio::test]
    async fn real_production_module_invalid_input_is_parsed_without_network() {
        let cache = tempfile::tempdir().unwrap();
        let helper = BrowserLogin {
            directory: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("tools/browser-helper"),
            timeout: Duration::from_secs(10),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        let mut command = helper.command();
        command
            .env("UV_NO_SYNC", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("UV_CACHE_DIR", cache.path().join("uv-cache"));
        // Both empty fields are rejected by helper parse_input before browser import
        // or any request. This exercises the REAL production module, not fake uv.
        let error = helper
            .execute(command, String::new(), String::new())
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "helper_input_invalid");
        assert_eq!(error.source, Some(ErrorSource::Helper));
        assert!(error.diagnostics.is_none());
    }

    #[tokio::test]
    async fn nonzero_structured_helper_errors_keep_safe_classification() {
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_secs(3),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        for (error, expected) in [
            ("invalid_credentials", "invalid_credentials"),
            ("challenge_or_block", "upstream_challenge"),
            ("browser_unavailable", "browser_unavailable"),
            ("invalid_input", "helper_input_invalid"),
            ("timeout", "upstream_timeout"),
            ("login_form_unavailable", "login_form_unavailable"),
            ("user_self_unverified", "upstream_session_unverified"),
            ("login_failed", "upstream_login_failed"),
            ("unrecognized-secret-error", "upstream_unexpected_response"),
        ] {
            let mut command = namespace_command();
            command.args(["python", "-c", &format!("import sys,json;sys.stdin.readline();print(json.dumps({{'ok':False,'error':'{error}'}}));sys.exit(1)")]);
            let safe = helper
                .execute(command, "fake".into(), "fake".into())
                .await
                .err()
                .unwrap();
            assert_eq!(safe.code, expected);
            assert_eq!(
                safe.source,
                Some(if expected == "invalid_credentials" {
                    ErrorSource::Credentials
                } else {
                    ErrorSource::Helper
                })
            );
            assert!(!safe.message.contains("unrecognized-secret-error"));
        }
        let mut command = namespace_command();
        command.args([
            "python",
            "-c",
            "import sys;sys.stdin.readline();print('{}');sys.exit(1)",
        ]);
        assert_eq!(
            helper
                .execute(command, "fake".into(), "fake".into())
                .await
                .err()
                .unwrap()
                .code,
            "upstream_unavailable"
        );
        let mut command = namespace_command();
        command.args(["python", "-c", "import sys,json;v=json.loads(sys.stdin.readline());assert set(v)=={'username','password','timeout_ms'};assert type(v['timeout_ms']) is int;print(json.dumps({'ok':True}));sys.exit(1)"]);
        assert_eq!(
            helper
                .execute(command, "fake".into(), "fake".into())
                .await
                .err()
                .unwrap()
                .code,
            "upstream_unavailable"
        );
    }

    #[tokio::test]
    async fn production_namespace_argv_stdin_stdout_and_loopback_network() {
        let directory = tempfile::tempdir().unwrap();
        let python = Command::new("python")
            .args(["-c", "import sys;print(sys.executable)"])
            .output()
            .unwrap();
        let python = String::from_utf8(python.stdout).unwrap();
        let uv = directory.path().join("uv");
        let script = format!(
            "#!{}\nimport sys,os,json,socket\nassert sys.argv[1:]==['run','--offline','--frozen','--no-sync','python','-B','-m','browser_helper']\nassert os.getpid()==1\nassert os.getuid()==1000\nv=json.loads(sys.stdin.readline()); assert v=={{'username':'fake-user','password':'fake-password','timeout_ms':3000}}\nassert not sys.stdin.read()\ns=socket.socket();s.bind(('127.0.0.1',0));s.listen();c=socket.create_connection(s.getsockname());p,_=s.accept();c.sendall(b'local');assert p.recv(5)==b'local'\nprint(json.dumps({{'ok':True,'api_user':'7','cookies':[{{'name':'session','value':'fake-cookie','domain':'anyrouter.top','path':'/','secure':True,'http_only':True,'expires':-1}}]}}))\n",
            python.trim()
        );
        std::fs::write(&uv, script).unwrap();
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o700)).unwrap();
        let helper = BrowserLogin {
            directory: directory.path().into(),
            timeout: Duration::from_secs(3),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        let mut command = helper.command();
        command.env(
            "PATH",
            format!(
                "{}:{}",
                directory.path().display(),
                std::env::var("PATH").unwrap()
            ),
        );
        let credentials = helper
            .execute(command, "fake-user".into(), "fake-password".into())
            .await
            .unwrap_or_else(|e| panic!("{}", e.code));
        assert_eq!(credentials.api_user, "7");
        assert_eq!(credentials.cookies[0].value, "fake-cookie");
    }

    #[tokio::test]
    async fn missing_namespace_tool_fails_closed_without_running_uv() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("must-not-run");
        let mut command = namespace_command();
        command.env("PATH", directory.path()).arg("uv").arg(&marker);
        let helper = BrowserLogin {
            directory: directory.path().into(),
            timeout: Duration::from_secs(1),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        assert_eq!(
            helper
                .execute(command, "fake".into(), "fake".into())
                .await
                .err()
                .unwrap()
                .code,
            "upstream_unavailable"
        );
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn namespace_missing_inner_runtime_fails_closed() {
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_secs(2),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        let mut command = namespace_command();
        command.arg("/definitely-absent-anyrouter-test-runtime");
        assert_eq!(
            helper
                .execute(command, "fake".into(), "fake".into())
                .await
                .err()
                .unwrap()
                .code,
            "upstream_unavailable"
        );
    }

    async fn namespace_escape_case(mode: &'static str) {
        let directory = tempfile::tempdir().unwrap();
        let pids_file = directory.path().join("escaped-pid");
        let release = directory.path().join("release");
        let host_proc = std::fs::File::open("/proc").unwrap();
        let fd = host_proc.as_raw_fd();
        let mut command = namespace_command();
        let code = format!(
            r#"import os,sys,time,json
sys.stdin.readline()
if os.fork()==0:
 os.setsid()
 if os.fork()!=0: os._exit(0)
 hostpid=os.readlink('/proc/self/fd/{fd}/self')
 open(sys.argv[1],'w').write(hostpid)
 os.close(1)
 while True: time.sleep(1)
while not os.path.exists(sys.argv[2]): time.sleep(.01)
print(json.dumps({{'ok':False,'error':'invalid_credentials'}}))
"#
        );
        command
            .args(["python", "-c", &code])
            .arg(&pids_file)
            .arg(&release);
        // Only tests retain a host proc descriptor to identify the deliberately
        // escaped descendant; production inherits no such descriptor.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_millis(if mode == "timeout" { 700 } else { 5000 }),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        let task =
            tokio::spawn(
                async move { helper.execute(command, "fake".into(), "fake".into()).await },
            );
        let hostpid = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pids_file)
                    && let Ok(pid) = text.parse::<i32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual namespace escaped process must start");
        // pidfd pins identity: readiness proves THIS host process exited, not
        // a later process that happened to reuse the PID.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, hostpid, 0) as i32 };
        assert!(raw >= 0);
        use std::os::fd::FromRawFd;
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
        match mode {
            "normal" => {
                std::fs::write(&release, b"go").unwrap();
                assert_eq!(
                    task.await.unwrap().err().unwrap().code,
                    "invalid_credentials"
                );
            }
            "timeout" => assert_eq!(task.await.unwrap().err().unwrap().code, "upstream_timeout"),
            "cancel" => {
                task.abort();
                assert!(task.await.err().unwrap().is_cancelled());
            }
            _ => unreachable!(),
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let mut poll = libc::pollfd {
                    fd: pidfd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut poll, 1, 0) } == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("namespace death must kill double-fork setsid descendant");
    }

    #[tokio::test]
    async fn namespace_normal_exit_kills_double_fork_setsid_descendant() {
        namespace_escape_case("normal").await;
    }
    #[tokio::test]
    async fn namespace_timeout_kills_double_fork_setsid_descendant() {
        namespace_escape_case("timeout").await;
    }
    #[tokio::test]
    async fn namespace_cancel_kills_double_fork_setsid_descendant() {
        namespace_escape_case("cancel").await;
    }

    #[test]
    fn parent_death_harness_child() {
        let Some(path) = std::env::var_os("AR_TEST_PARENT_DEATH_PID_FILE") else {
            return;
        };
        let host_proc = std::fs::File::open("/proc").unwrap();
        let fd = host_proc.as_raw_fd();
        let mut command = namespace_command();
        command.args(["python", "-c", &format!("import os,sys,time;open(sys.argv[1],'w').write(os.readlink('/proc/self/fd/{fd}/self'));time.sleep(60)")]).arg(path)
            .stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        parent_death_signal(&mut command);
        let _ = command.spawn().unwrap().wait();
    }

    #[tokio::test]
    async fn abrupt_parent_death_kills_namespace_pid1() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("namespace-pid1");
        let mut leader = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "helper::tests::parent_death_harness_child"])
            .env("AR_TEST_PARENT_DEATH_PID_FILE", &pid_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pid_file)
                    && let Ok(pid) = text.parse::<i32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("parent-death local harness namespace must start");
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
        assert!(raw >= 0);
        use std::os::fd::FromRawFd;
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
        // Kill only this test-owned backend-leader simulation, never arbitrary PIDs.
        leader.kill().unwrap();
        leader.wait().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let mut poll = libc::pollfd {
                    fd: pidfd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut poll, 1, 0) } == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("PDEATHSIG and unshare kill-child must terminate namespace PID1");
    }

    #[tokio::test]
    async fn helper_errors_are_allowlisted_and_stdout_bounded() {
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_secs(2),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        for (output, expected) in [
            (
                r#"{'ok':False,'error':'challenge_or_block'}"#,
                "upstream_challenge",
            ),
            (
                r#"{'ok':False,'error':'invalid_credentials'}"#,
                "invalid_credentials",
            ),
            (
                r#"{'ok':False,'error':'secret-password-cookie'}"#,
                "upstream_unexpected_response",
            ),
        ] {
            let mut command = Command::new("python");
            command.args([
                "-c",
                &format!("import json,sys; sys.stdin.readline(); print(json.dumps({output}))"),
            ]);
            assert_eq!(
                helper
                    .execute(command, "user".into(), "password".into())
                    .await
                    .err()
                    .unwrap()
                    .code,
                expected
            );
        }
        let mut command = Command::new("python");
        command.args([
            "-c",
            "import sys; sys.stdin.readline(); sys.stdout.write('x'*300000)",
        ]);
        assert_eq!(
            helper
                .execute(command, "user".into(), "password".into())
                .await
                .err()
                .unwrap()
                .code,
            "upstream_unexpected_response"
        );
    }

    #[tokio::test]
    async fn helper_deadline_kills_descendants() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_millis(400),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        let mut command = Command::new("python");
        command.args(["-c", "import os,subprocess,sys,time; sys.stdin.readline(); p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']); open(sys.argv[1],'w').write(f'{os.getpid()} {p.pid}'); time.sleep(60)"])
            .arg(&pid_file);
        assert_eq!(
            helper
                .execute(command, "user".into(), "password".into())
                .await
                .err()
                .unwrap()
                .code,
            "upstream_timeout"
        );
        let pids = std::fs::read_to_string(pid_file).unwrap();
        let mut pids = pids.split_whitespace();
        wait_dead(pids.next().unwrap(), true).await;
        wait_dead(pids.next().unwrap(), false).await;
    }

    async fn wait_dead(pid: &str, reaped: bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Err(_) => break,
                    Ok(stat)
                        if !reaped
                            && stat
                                .split(')')
                                .nth(1)
                                .is_some_and(|tail| tail.trim_start().starts_with('Z')) =>
                    {
                        break;
                    }
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .expect("process must be killed/reaped within bound");
    }

    #[tokio::test]
    async fn helper_normal_exit_kills_group_before_reaping_leader() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pids");
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_secs(3),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        let mut command = Command::new("python");
        command.args(["-c", "import os,subprocess,sys,json; sys.stdin.readline(); p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'],stdout=subprocess.DEVNULL); open(sys.argv[1],'w').write(f'{os.getpid()} {p.pid}'); print(json.dumps({'ok':False,'error':'invalid_credentials'}))"])
            .arg(&pid_file);
        assert_eq!(
            helper
                .execute(command, "user".into(), "password".into())
                .await
                .err()
                .unwrap()
                .code,
            "invalid_credentials"
        );
        let pids = std::fs::read_to_string(pid_file).unwrap();
        let mut pids = pids.split_whitespace();
        wait_dead(pids.next().unwrap(), true).await;
        wait_dead(pids.next().unwrap(), false).await;
    }

    #[tokio::test]
    async fn helper_task_cancel_kills_group_and_reaps_leader() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pids");
        let helper = BrowserLogin {
            directory: PathBuf::new(),
            timeout: Duration::from_secs(60),
            diagnostics: false,
            executable: None,
            container_mode: false,
        };
        let mut command = Command::new("python");
        command.args(["-c", "import os,subprocess,sys,time; sys.stdin.readline(); p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']); open(sys.argv[1],'w').write(f'{os.getpid()} {p.pid}'); time.sleep(60)"])
            .arg(&pid_file);
        let task = tokio::spawn(async move {
            helper
                .execute(command, "user".into(), "password".into())
                .await
        });
        let pids = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(pids) = std::fs::read_to_string(&pid_file)
                    && pids.split_whitespace().count() == 2
                {
                    break pids;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(
            task.await
                .err()
                .expect("aborted helper task")
                .is_cancelled()
        );
        let mut pids = pids.split_whitespace();
        wait_dead(pids.next().unwrap(), true).await;
        wait_dead(pids.next().unwrap(), false).await;
    }
}
#[test]
fn container_command_is_explicit_and_native_command_keeps_namespace() {
    let mut login = BrowserLogin {
        directory: PathBuf::from("/helper"),
        timeout: Duration::from_secs(1),
        diagnostics: false,
        executable: Some(PathBuf::from("/python")),
        container_mode: true,
    };
    assert_eq!(login.command().get_program(), "/python");
    assert_eq!(
        login.command().get_args().collect::<Vec<_>>(),
        ["-B", "-m", "browser_helper"]
    );
    login.executable = None;
    assert_eq!(login.command().get_program(), "uv");
    login.container_mode = false;
    assert_eq!(login.command().get_program(), "unshare");
}
