//! Offline host tests of the production worker, not Docker validation.
#[allow(dead_code)]
#[path = "../src/supervisor.rs"]
mod supervisor;

use std::{
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
const EXE: &str = env!("CARGO_BIN_EXE_anyrouter-manager-backend");

fn python(script: &str, directory: &Path) -> Command {
    let mut command = Command::new("python");
    command.args(["-B", "-c", script]).current_dir(directory);
    command
}

async fn marker(path: &Path) -> i32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("synthetic marker")
}

fn pin(pid: i32) -> OwnedFd {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    assert!(fd >= 0);
    unsafe { OwnedFd::from_raw_fd(fd as i32) }
}

fn dead(fd: &OwnedFd) -> bool {
    let mut p = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut p, 1, 0) > 0 }
}

fn reaped(fd: &OwnedFd) -> bool {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe {
        libc::poll(&mut poll, 1, 0);
    }
    poll.revents & libc::POLLHUP != 0
}

// Inspect only the known job's descendant tree, including adopted Crashpad
// processes. Never scan by UID, read environments, or kill through numeric PIDs.
fn pinned_tree(root: i32) -> Vec<(OwnedFd, String)> {
    let mut pending = vec![root];
    let mut seen = std::collections::HashSet::new();
    let mut pinned = Vec::new();
    while let Some(pid) = pending.pop() {
        if !seen.insert(pid) {
            continue;
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            continue; // A transient child exited before it could be pinned.
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for task in tasks.flatten() {
                if let Ok(children) = std::fs::read_to_string(task.path().join("children")) {
                    pending.extend(
                        children
                            .split_whitespace()
                            .filter_map(|s| s.parse::<i32>().ok()),
                    );
                }
            }
        }
        pinned.push((fd, name));
    }
    pinned
}

const ESCAPE: &str = r#"
import os,sys,time
sys.stdin.readline()
open('supervisor','w').write(str(os.getppid()))
if os.fork()==0:
    os.setsid()
    if os.fork()!=0: os._exit(0)
    open('escaped','w').write(str(os.getpid()))
    while True: time.sleep(1)
os.wait()
while not os.path.exists('escaped'): time.sleep(.005)
while not os.path.exists('finish'): time.sleep(.005)
print('{}',flush=True)
"#;

#[tokio::test]
async fn normal_timeout_cancel_reap_escaped_descendants_and_preserve_bystander() {
    let mut unrelated = Command::new("sleep").arg("60").spawn().unwrap();
    for mode in ["normal", "timeout", "cancel", "sigterm"] {
        let directory = tempfile::tempdir().unwrap();
        let gate = Arc::new(Semaphore::new(1));
        let permit = gate.clone().acquire_owned().await.unwrap();
        let (mut worker, mut input, output) = supervisor::spawn(
            Path::new(EXE),
            &python(ESCAPE, directory.path()),
            Some(permit),
        )
        .unwrap();
        input.write_all(b"{}\n").unwrap();
        drop(input);
        let escaped = pin(marker(&directory.path().join("escaped")).await);
        assert!(!dead(&escaped));
        match mode {
            "normal" => {
                std::fs::write(directory.path().join("finish"), b"go").unwrap();
                assert!(worker.exited_successfully().await.unwrap());
            }
            "timeout" => {
                assert!(
                    tokio::time::timeout(Duration::from_millis(30), worker.exited_successfully())
                        .await
                        .is_err()
                );
                worker.close().await;
            }
            "sigterm" => {
                let parent: i32 = std::fs::read_to_string(directory.path().join("supervisor"))
                    .unwrap()
                    .parse()
                    .unwrap();
                let fd = pin(parent);
                assert_eq!(
                    unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            fd.as_raw_fd(),
                            libc::SIGTERM,
                            0,
                            0,
                        )
                    },
                    0
                );
                assert!(!worker.exited_successfully().await.unwrap());
            }
            _ => {
                drop(worker);
            }
        }
        let _next = tokio::time::timeout(Duration::from_secs(10), gate.acquire())
            .await
            .unwrap()
            .unwrap();
        assert!(
            reaped(&escaped),
            "ACK+permit release must follow descendant reap, not just exit"
        );
        drop(output);
        assert!(unrelated.try_wait().unwrap().is_none());
    }
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[tokio::test]
async fn fork_reap_interleaving_and_two_generations_do_not_overlap() {
    let directory = tempfile::tempdir().unwrap();
    let gate = Arc::new(Semaphore::new(1));
    let mut previous = None;
    for generation in 0..2 {
        let permit = tokio::time::timeout(Duration::from_secs(10), gate.clone().acquire_owned())
            .await
            .unwrap()
            .unwrap();
        if let Some(fd) = &previous {
            assert!(dead(fd));
        }
        let script = format!(
            r#"
import os,time
for i in range(50):
    if os.fork()==0:
        os.setsid()
        if os.fork()!=0: os._exit(0)
        time.sleep(.02)
        os._exit(0)
    os.wait()
open('generation-{generation}','w').write(str(os.getpid()))
while True: time.sleep(1)
"#
        );
        let (worker, input, output) = supervisor::spawn(
            Path::new(EXE),
            &python(&script, directory.path()),
            Some(permit),
        )
        .unwrap();
        previous = Some(pin(marker(
            &directory.path().join(format!("generation-{generation}")),
        )
        .await));
        assert!(gate.try_acquire().is_err());
        drop((worker, input, output));
    }
    let _permit = tokio::time::timeout(Duration::from_secs(10), gate.acquire())
        .await
        .unwrap()
        .unwrap();
    assert!(dead(previous.as_ref().unwrap()));
}

#[tokio::test]
async fn blocked_input_large_output_and_eof_remain_cancellable() {
    for script in [
        "import time;time.sleep(60)",
        "import os,time;os.write(1,b'x'*1048576);time.sleep(60)",
        "import sys;assert sys.stdin.read()==''",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (mut worker, mut input, mut output) =
            supervisor::spawn(Path::new(EXE), &python(script, directory.path()), None).unwrap();
        if script.starts_with("import sys") {
            drop(input);
            assert!(worker.exited_successfully().await.unwrap());
        } else {
            let writer = if script.starts_with("import time") {
                Some(std::thread::spawn(move || {
                    input.write_all(&vec![b'x'; 1024 * 1024])
                }))
            } else {
                drop(input);
                let mut bytes = vec![0; 256 * 1024 + 1];
                output.read_exact(&mut bytes).unwrap();
                assert_eq!(bytes.len(), 256 * 1024 + 1);
                None
            };
            tokio::time::sleep(Duration::from_millis(40)).await;
            worker.close().await;
            if let Some(writer) = writer {
                assert!(writer.join().unwrap().is_err());
            }
        }
        drop(output);
    }
}

// Isolated process harnesses: fatal behavior must not take down the test runner.
#[tokio::test]
async fn isolated_harness() {
    let Ok(mode) = std::env::var("SUPERVISOR_TEST_MODE") else {
        return;
    };
    let directory = std::env::var_os("SUPERVISOR_TEST_DIRECTORY").unwrap();
    let directory = Path::new(&directory);
    let gate = Arc::new(Semaphore::new(1));
    let permit = gate.clone().acquire_owned().await.unwrap();
    let script = if mode == "fatal" {
        "import os,signal;os.kill(os.getppid(),signal.SIGKILL)"
    } else {
        ESCAPE
    };
    let fake = directory.join("fake-worker");
    let executable = if mode == "missing_ack" || mode == "stalled" {
        use std::os::unix::fs::PermissionsExt;
        let python = Command::new("python")
            .args(["-c", "import sys;print(sys.executable)"])
            .output()
            .unwrap();
        let python = String::from_utf8(python.stdout).unwrap();
        let body = if mode == "stalled" {
            "import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)"
        } else {
            "pass"
        };
        std::fs::write(&fake, format!("#!{}\n{}\n", python.trim(), body)).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        fake.as_path()
    } else {
        Path::new(EXE)
    };
    let (mut worker, mut input, output) =
        supervisor::spawn(executable, &python(script, directory), Some(permit)).unwrap();
    let _ = input.write_all(b"{}\n");
    if mode == "stalled" {
        worker.close().await;
    }
    let _ = worker.exited_successfully().await;
    let _next = gate.acquire().await.unwrap();
    std::fs::write(directory.join("next-generation"), b"unexpected").unwrap();
    drop(output);
}

fn harness(mode: &str, dir: &Path) -> std::process::Child {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "isolated_harness", "--nocapture"])
        .env("SUPERVISOR_TEST_MODE", mode)
        .env("SUPERVISOR_TEST_DIRECTORY", dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if mode == "no_pidfd" {
        // Add a restrictive test-only syscall filter, never relax the host policy.
        // The worker must fail closed when pidfd_open is unavailable.
        unsafe {
            command.pre_exec(|| {
                let mut filter = [
                    libc::sock_filter {
                        code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                        jt: 0,
                        jf: 0,
                        k: 0,
                    },
                    libc::sock_filter {
                        code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                        jt: 0,
                        jf: 1,
                        k: libc::SYS_pidfd_open as u32,
                    },
                    libc::sock_filter {
                        code: (libc::BPF_RET | libc::BPF_K) as u16,
                        jt: 0,
                        jf: 0,
                        k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
                    },
                    libc::sock_filter {
                        code: (libc::BPF_RET | libc::BPF_K) as u16,
                        jt: 0,
                        jf: 0,
                        k: libc::SECCOMP_RET_ALLOW,
                    },
                ];
                let program = libc::sock_fprog {
                    len: filter.len() as u16,
                    filter: filter.as_mut_ptr(),
                };
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0
                    || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command.spawn().unwrap()
}

#[tokio::test]
async fn worker_sigkill_is_fatal_and_never_allows_next_generation() {
    for mode in ["fatal", "missing_ack", "no_pidfd"] {
        let directory = tempfile::tempdir().unwrap();
        let mut process = harness(mode, directory.path());
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = process.try_wait().unwrap() {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(status.code(), Some(70));
        assert!(!directory.path().join("next-generation").exists());
    }
}

#[tokio::test]
async fn stalled_worker_is_fatal_without_permit_release() {
    // The intentional fault must be enclosed in a test PID namespace so its
    // uncooperative fake worker is removed with the isolated service process.
    let directory = tempfile::tempdir().unwrap();
    let mut process = Command::new("unshare")
        .args([
            "--user",
            "--map-current-user",
            "--pid",
            "--fork",
            "--kill-child=KILL",
            "--mount-proc",
            "--",
        ])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "isolated_harness"])
        .env("SUPERVISOR_TEST_MODE", "stalled")
        .env("SUPERVISOR_TEST_DIRECTORY", directory.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if let Some(status) = process.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status.code(), Some(70));
    assert!(!directory.path().join("next-generation").exists());
}

#[tokio::test]
#[ignore = "requires the locked browser dev shell and existing helper venv; about:blank only"]
async fn actual_cloakbrowser_normal_and_timeout_cleanup() {
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/browser-helper");
    for normal in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let marker_path = directory.path().join("browser-started");
        let finish_path = directory.path().join("finish");
        let script = r#"
import asyncio,os,sys
from browser_helper.core import browser_launcher
async def main():
    browser=await browser_launcher()(headless=True)
    context=await browser.new_context()
    await context.route('**/*',lambda route:route.abort())
    page=await context.new_page()
    await page.goto('about:blank')
    assert await page.evaluate('1+1')==2
    open(sys.argv[1],'w').write(str(os.getpid()))
    while not os.path.exists(sys.argv[2]): await asyncio.sleep(.01)
    await context.close()
    await browser.close()
asyncio.run(main())
"#;
        let mut command = Command::new(helper.join(".venv/bin/python"));
        command
            .args(["-B", "-c", script])
            .arg(&marker_path)
            .arg(&finish_path)
            .current_dir(&helper);
        let (mut worker, input, output) =
            supervisor::spawn(Path::new(EXE), &command, None).unwrap();
        drop(input);
        let pid = tokio::time::timeout(Duration::from_secs(40), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&marker_path)
                    && let Ok(pid) = text.parse::<i32>()
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let parent: i32 = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let tree = pinned_tree(parent);
        assert!(tree.len() > 3, "real browser process tree must be observed");
        assert!(
            tree.iter().any(|(_, name)| name.contains("crashpad")),
            "real Crashpad process must be observed"
        );
        // Every observed browser/Crashpad identity must have exited before the
        // ACK, which additionally attests the sole reaper reached ECHILD.
        if normal {
            std::fs::write(&finish_path, b"go").unwrap();
            assert!(worker.exited_successfully().await.unwrap());
        } else {
            assert!(
                tokio::time::timeout(Duration::from_millis(50), worker.exited_successfully())
                    .await
                    .is_err()
            );
            worker.close().await;
        }
        assert!(
            tree.iter().all(|(fd, _)| reaped(fd)),
            "browser/Crashpad must be reaped before ACK"
        );
        drop(output);
    }
}

#[tokio::test]
async fn parent_sigkill_still_cleans_escaped_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let mut process = harness("parent", directory.path());
    let escaped = pin(marker(&directory.path().join("escaped")).await);
    let worker_pid = marker(&directory.path().join("supervisor")).await;
    let worker = pin(worker_pid);
    process.kill().unwrap();
    process.wait().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !reaped(&escaped) || !dead(&worker) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!directory.path().join("next-generation").exists());
}

#[tokio::test]
async fn readiness_probe_starts_no_helper_and_does_not_need_unshare() {
    let (mut worker, input, output) =
        supervisor::spawn(Path::new(EXE), &Command::new("--probe"), None).unwrap();
    drop((input, output));
    assert!(worker.exited_successfully().await.unwrap());
}

#[tokio::test]
async fn control_and_ack_descriptors_are_not_inherited_by_helper() {
    let directory = tempfile::tempdir().unwrap();
    let script = r#"
import os
for name in os.listdir('/proc/self/fd'):
    fd=int(name)
    if fd<=2: continue
    try: os.fstat(fd)
    except OSError: continue
    raise AssertionError('unexpected inherited descriptor')
"#;
    let (mut worker, input, output) =
        supervisor::spawn(Path::new(EXE), &python(script, directory.path()), None).unwrap();
    drop(input);
    assert!(worker.exited_successfully().await.unwrap());
    drop(output);
}
