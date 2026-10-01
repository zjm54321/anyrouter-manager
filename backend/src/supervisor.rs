//! Container-only lifecycle management, NOT a namespace-equivalent sandbox.
//! Requires a private container PID namespace: host-PID deployments are forbidden.
//! A lost supervisor is fatal to the service; container teardown is the last fence.
use std::{
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::{ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, oneshot};

const WORKER: &str = "--internal-browser-worker";
const LIMIT: Duration = Duration::from_secs(8);

fn fd_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: valid two-element output buffer; descriptors become uniquely owned.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn pidfd(pid: libc::pid_t) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

fn capability() -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = pidfd(unsafe { libc::getpid() })?;
    // Signal zero verifies pidfd_send_signal is permitted without signaling anyone.
    if unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd.as_raw_fd(), 0, 0, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    direct_children()?;
    Ok(())
}

fn direct_children() -> io::Result<Vec<libc::pid_t>> {
    let pid = unsafe { libc::getpid() };
    std::fs::read_to_string(format!("/proc/self/task/{pid}/children"))?
        .split_whitespace()
        .map(|s| s.parse().map_err(|_| io::ErrorKind::InvalidData.into()))
        .collect()
}

fn cleanup() -> io::Result<()> {
    let start = Instant::now();
    loop {
        // No other thread or code may wait/reap in this worker. Even zombies pin
        // their PID until this sole reaper runs, so pidfd_open cannot hit PID reuse.
        for pid in direct_children()? {
            let fd = pidfd(pid)?;
            let result = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    libc::SIGKILL,
                    0,
                    0,
                )
            };
            if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return Err(io::Error::last_os_error());
            }
        }
        loop {
            let result = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
            if result > 0 {
                continue;
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    return Ok(());
                }
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(error);
            }
            break;
        }
        if start.elapsed() >= LIMIT {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn worker(args: &[std::ffi::OsString]) -> io::Result<()> {
    if args.len() < 4 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let number = |index: usize| -> io::Result<i32> {
        args[index]
            .to_str()
            .and_then(|s| s.parse().ok())
            .ok_or(io::ErrorKind::InvalidInput.into())
    };
    let parent = number(0)?;
    let control_fd = number(1)?;
    let ack_fd = number(2)?;
    if control_fd < 3 || ack_fd < 3 || control_fd == ack_fd {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // Set CLOEXEC before starting a helper: grandchildren must never keep these alive.
    for fd in [control_fd, ack_fd] {
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let control = unsafe { OwnedFd::from_raw_fd(control_fd) };
    let mut ack = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(ack_fd) });
    let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    unsafe {
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, libc::SIGTERM);
    }
    if unsafe { libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let signal_fd = unsafe { libc::signalfd(-1, &mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
    if signal_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let signals = unsafe { OwnedFd::from_raw_fd(signal_fd) };
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } < 0 {
        return Err(io::Error::last_os_error());
    }
    capability()?;
    let mut success = false;
    if unsafe { libc::getppid() } == parent {
        if args[3] == "--probe" {
            success = true; // No browser or Python is started by readiness.
        } else {
            let mut command = Command::new(&args[3]);
            command
                .args(&args[4..])
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(move || {
                    if libc::sigprocmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut()) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            if let Ok(child) = command.spawn() {
                let pid = child.id() as i32;
                // Child has no Drop reaper. This loop and cleanup are the ONLY wait owners.
                drop(child);
                loop {
                    let mut status = 0;
                    let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                    if result == pid {
                        success = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
                        break;
                    }
                    if result < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    let mut polls = [
                        libc::pollfd {
                            fd: control.as_raw_fd(),
                            events: libc::POLLIN | libc::POLLHUP,
                            revents: 0,
                        },
                        libc::pollfd {
                            fd: signals.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                    ];
                    let result = unsafe { libc::poll(polls.as_mut_ptr(), 2, 10) };
                    if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR)
                    {
                        return Err(io::Error::last_os_error());
                    }
                    if polls.iter().any(|p| p.revents != 0) {
                        break;
                    }
                }
            }
        }
    }
    cleanup()?; // ACK is impossible until the sole reaper has observed ECHILD.
    ack.write_all(&[u8::from(success)])?;
    Ok(())
}

/// Must run before constructing Tokio or starting any threads.
pub fn dispatch() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|s| s == WORKER) {
        std::process::exit(if worker(&args[2..]).is_ok() { 0 } else { 70 });
    }
}

fn fatal() -> ! {
    // No command, credentials, environment, or child output is reflected here.
    eprintln!("Fatal browser supervisor failure; private container teardown required.");
    std::process::exit(70)
}

pub struct Managed {
    control: Option<OwnedFd>,
    cancelled: Arc<AtomicBool>,
    completion: Option<oneshot::Receiver<bool>>,
    success: Option<bool>,
}

impl Managed {
    pub async fn exited_successfully(&mut self) -> io::Result<bool> {
        if let Some(success) = self.success {
            return Ok(success);
        }
        let success = self
            .completion
            .as_mut()
            .ok_or(io::ErrorKind::BrokenPipe)?
            .await
            .unwrap_or_else(|_| fatal());
        self.completion.take();
        self.success = Some(success);
        Ok(success)
    }
    pub async fn close(&mut self) {
        self.cancel();
        if self.exited_successfully().await.is_err() {
            fatal();
        }
    }
    fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.control.take();
    }
}
impl Drop for Managed {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn spawn(
    executable: &std::path::Path,
    helper: &Command,
    permit: Option<OwnedSemaphorePermit>,
) -> io::Result<(Managed, ChildStdin, ChildStdout)> {
    let (control_read, control_write) = fd_pipe()?;
    let (ack_read, ack_write) = fd_pipe()?;
    let parent = unsafe { libc::getpid() };
    let control_fd = control_read.as_raw_fd();
    let ack_fd = ack_write.as_raw_fd();
    let mut command = Command::new(executable);
    command
        .arg(WORKER)
        .arg(parent.to_string())
        .arg(control_fd.to_string())
        .arg(ack_fd.to_string())
        .arg(helper.get_program())
        .args(helper.get_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(dir) = helper.get_current_dir() {
        command.current_dir(dir);
    }
    for (key, value) in helper.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    unsafe {
        command.pre_exec(move || {
            let mut mask = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGTERM);
            if libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()) < 0
                || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) < 0
                || libc::fcntl(control_fd, libc::F_SETFD, 0) < 0
                || libc::fcntl(ack_fd, libc::F_SETFD, 0) < 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(70);
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    drop(control_read);
    drop(ack_write);
    let stdin = child.stdin.take().expect("piped input");
    let stdout = child.stdout.take().expect("piped output");
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancellation = cancelled.clone();
    let (tx, rx) = oneshot::channel();
    // A dedicated monitor is independent of Tokio task cancellation/IO stalls.
    // It owns both the unique worker wait and the permit until verified ACK+reap.
    // Keep a second owner until monitor creation succeeds. A thread-allocation
    // failure must not release the permit while the already spawned worker lives.
    let permit = Arc::new(permit);
    let monitor_permit = permit.clone();
    std::thread::Builder::new()
        .name("browser-reaper".into())
        .spawn(move || {
            let _permit = monitor_permit;
            let mut deadline = None;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let mut bytes = [255; 2];
                        let mut ack = std::fs::File::from(ack_read);
                        if !status.success() || ack.read(&mut bytes).ok() != Some(1) || bytes[0] > 1
                        {
                            fatal();
                        }
                        let _ = tx.send(bytes[0] == 1);
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => fatal(),
                }
                if cancellation.load(Ordering::SeqCst) {
                    let start = deadline.get_or_insert_with(Instant::now);
                    if start.elapsed() >= LIMIT + Duration::from_secs(1) {
                        fatal();
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
        .unwrap_or_else(|_| fatal());
    Ok((
        Managed {
            control: Some(control_write),
            cancelled,
            completion: Some(rx),
            success: None,
        },
        stdin,
        stdout,
    ))
}

pub async fn available() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Ok((mut managed, stdin, stdout)) = spawn(&exe, &Command::new("--probe"), None) else {
        return false;
    };
    drop((stdin, stdout));
    let result = tokio::time::timeout(Duration::from_secs(2), managed.exited_successfully()).await;
    managed.close().await;
    result.is_ok_and(|r| r.unwrap_or(false))
}
