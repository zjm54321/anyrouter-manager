"""Offline observer of the exact image's Rust worker, before container deletion.

Protocol mirrors backend/src/supervisor.rs: worker argv is parent PID, control
read FD, ACK write FD, executable, args. ACK 1 = normal helper success, 0 =
cancel/failure; either requires ECHILD and worker exit 0. No account is used.
This probes the production worker, not the Rust parent's semaphore/fatal policy
(those remain separate backend native tests). Never treat Docker rm as evidence.
"""
import array
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time

BACKEND = "/app/anyrouter-manager-backend"
PYTHON = "/app/tools/browser-helper/.venv/bin/python"
WORKER = "--internal-browser-worker"
MODES = ["normal", "timeout", "cancel", "termination", "parent_death"]
BROWSER_MODES = ["normal", "timeout"]
SYNTHETIC = r'''
import json,os,time
from pathlib import Path
assert json.loads(input()) == {}
# Rust must close control/ACK on exec. Ignore the ephemeral listdir descriptor.
for fd in map(int, os.listdir('/proc/self/fd')):
    if fd <= 2: continue
    try: os.fstat(fd)
    except OSError: continue
    raise AssertionError('inherited descriptor')
if os.fork() == 0:
    os.setsid()
    if os.fork() != 0: os._exit(0)
    Path('escaped').write_text(str(os.getpid()))
    while True: time.sleep(1)
os.wait()
while not Path('escaped').exists(): time.sleep(.005)
Path('ready').write_text(str(os.getpid()))
while not Path('finish').exists(): time.sleep(.005)
print('{"fixture":"done"}',flush=True)
'''


def wait_until(predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while not predicate():
        if time.monotonic() >= deadline:
            raise TimeoutError("fixed probe deadline")
        time.sleep(.01)


def reaped(fd):
    poll = select.poll()
    poll.register(fd, select.POLLIN)
    return any(events & select.POLLHUP for _, events in poll.poll(0))


def descendants(root):
    """Only the known job's children, never a UID or whole-system process scan."""
    pending, seen, result = [root], set(), []
    while pending:
        pid = pending.pop()
        if pid in seen:
            continue
        seen.add(pid)
        try:
            fd = os.pidfd_open(pid)
        except ProcessLookupError:
            continue
        try:
            name = Path(f"/proc/{pid}/comm").read_text().strip()
            for task in Path(f"/proc/{pid}/task").iterdir():
                try:
                    pending.extend(map(int, (task / "children").read_text().split()))
                except FileNotFoundError:
                    pass
        except FileNotFoundError:
            name = ""
        if pid == root:
            os.close(fd)
        else:
            result.append((fd, name))
    return result


def start_worker(helper, directory):
    cr, cw = os.pipe2(os.O_CLOEXEC | os.O_NONBLOCK)
    ar, aw = os.pipe2(os.O_CLOEXEC | os.O_NONBLOCK)
    try:
        child = subprocess.Popen(
            [BACKEND, WORKER, str(os.getpid()), str(cr), str(aw), *helper],
            pass_fds=(cr, aw), cwd=directory, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    except BaseException:
        os.close(cw)
        os.close(ar)
        raise
    finally:
        os.close(cr)
        os.close(aw)
    if helper != ["--probe"]:
        child.stdin.write(b"{}\n")
    child.stdin.close()
    return child, cw, ar


def accept_ack(ack, owned, expected):
    wait_until(lambda: bool(select.select([ack], [], [], 0)[0]))
    assert os.read(ack, 2) == expected, "missing/invalid cleanup ACK"
    # Check immediately on receipt, not after Docker stop or an extra cleanup wait.
    assert owned and all(reaped(fd) for fd, _ in owned), "ACK before descendant reap"


def capability():
    child, control, ack = start_worker(["--probe"], "/tmp")
    try:
        status = child.wait(timeout=5)
        return status == 0 and os.read(ack, 2) == b"\x01"
    finally:
        os.close(control)
        os.close(ack)
        child.stdout.close()


def bridge(fd, directory):
    """Dedicated worker parent. Transfer observation FDs, then await our SIGKILL.

    The outer observer keeps control open; parent death therefore tests PDEATHSIG
    rather than accidentally passing because the control writer disappeared.
    """
    with socket.socket(fileno=fd) as channel:
        child, control, ack = start_worker([PYTHON, "-B", "fixture.py"], directory)
        fds = array.array("i", [control, ack, child.stdout.fileno()])
        channel.sendmsg([str(child.pid).encode()], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, fds)])
        os.close(control)
        os.close(ack)
        child.stdout.close()
    while True:
        signal.pause()


def lifecycle(mode, browser=False):
    with tempfile.TemporaryDirectory(prefix="worker-probe-", dir="/tmp") as directory:
        directory = Path(directory)
        (directory / "fixture.py").write_text(SYNTHETIC)
        child = parent = None
        control = ack = output = worker_fd = parent_fd = None
        owned = []
        try:
            if mode == "parent_death":
                outer, inner = socket.socketpair()
                with outer, inner:
                    outer.settimeout(5)
                    parent = subprocess.Popen([sys.executable, "-B", __file__, "--bridge",
                        str(inner.fileno()), str(directory)], pass_fds=(inner.fileno(),),
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    parent_fd = os.pidfd_open(parent.pid)
                    message, ancillary, flags, _ = outer.recvmsg(64, socket.CMSG_SPACE(3 * array.array("i").itemsize))
                    assert not flags & socket.MSG_CTRUNC
                    fds = array.array("i")
                    for level, kind, data in ancillary:
                        assert (level, kind) == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
                        fds.frombytes(data)
                    assert len(fds) == 3
                    control, ack, output = fds
                    worker_pid = int(message)
            else:
                helper = [PYTHON, "-B", "/tmp/browser_probe.py" if browser else "fixture.py"]
                child, control, ack = start_worker(helper, directory)
                worker_pid = child.pid
            worker_fd = os.pidfd_open(worker_pid)
            wait_until(lambda: (directory / "ready").exists(), 40 if browser else 5)
            owned = descendants(worker_pid)
            assert len(owned) >= (4 if browser else 2), "job tree not observed"
            if browser:
                assert any("crashpad" in name for _, name in owned), "Crashpad not observed"
            if mode == "normal":
                (directory / "finish").write_text("go")
            elif mode == "timeout":
                assert not select.select([ack], [], [], .05)[0], "job exited before deadline"
                os.close(control)
                control = None
            elif mode == "cancel":
                os.close(control)
                control = None
            elif mode == "termination":
                signal.pidfd_send_signal(worker_fd, signal.SIGTERM)
            elif mode == "parent_death":
                signal.pidfd_send_signal(parent_fd, signal.SIGKILL)
                assert parent.wait(timeout=5) == -signal.SIGKILL
            else:
                raise AssertionError("unknown mode")
            accept_ack(ack, owned, b"\x01" if mode == "normal" else b"\x00")
            if child:
                assert child.wait(timeout=5) == 0
                assert len(child.stdout.read(256 * 1024 + 1)) <= 256 * 1024
            # On parent death tini must reap the orphan worker in this container.
            wait_until(lambda: reaped(worker_fd))
        finally:
            # Failure still propagates. Closing control asks Rust to clean up; no
            # observer kill of descendants or Docker rm can turn failure into pass.
            if control is not None:
                os.close(control)
            if child:
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    pass
                child.stdout.close()
            if parent and parent.poll() is None:
                signal.pidfd_send_signal(parent_fd, signal.SIGKILL)
                parent.wait(timeout=5)
            for fd in [ack, output, worker_fd, parent_fd, *(fd for fd, _ in owned)]:
                if fd is not None:
                    os.close(fd)


def main():
    stage = "capability"
    try:
        if not capability():
            print(json.dumps({"supervisor_probe": "blocked", "stage": stage}))
            return 2
        if sys.argv[1:] == ["--capability"]:
            print(json.dumps({"supervisor_probe": "passed", "stage": stage}))
            return 0
        for mode in MODES:
            stage = "synthetic_" + mode
            lifecycle(mode)
        for mode in BROWSER_MODES:
            stage = "browser_" + mode
            lifecycle(mode, browser=True)
    except Exception:
        print(json.dumps({"supervisor_probe": "blocked" if stage == "capability" else "failed", "stage": stage}))
        return 2 if stage == "capability" else 1
    print(json.dumps({"supervisor_probe": "passed", "modes": MODES,
        "browser_modes": BROWSER_MODES, "identity": "pidfd", "ack_after_reap": True,
        "before_container_removal": True, "production_worker": True}))
    return 0


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--bridge":
        bridge(int(sys.argv[2]), sys.argv[3])
    else:
        sys.exit(main())
