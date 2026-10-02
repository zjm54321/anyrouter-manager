"""Docker-only offline smoke; no real accounts and no raw output/log reflection."""
import json
import os
from pathlib import Path
import secrets
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
PROBES = ("http_probe.py", "supervisor_probe.py", "browser_probe.py")
# Execute as the image's default UID, inside its writable /tmp mount. Docker's
# archive-copy path is deliberately not used for a read-only root filesystem.
# Only public probe SOURCE travels here; the synthetic root uses a later stdin.
PROBE_WRITER = r'''
import os,sys
assert sys.argv[1] in ("http_probe.py", "supervisor_probe.py", "browser_probe.py")
source = sys.stdin.buffer.read(65537)
assert 0 < len(source) <= 65536
fd = os.open("/tmp/" + sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
with os.fdopen(fd, "wb") as stream:
    stream.write(source)
'''
STATE_FORMAT = '{{.State.Status}} {{.State.ExitCode}}'


class SmokeFailure(Exception):
    """Fixed reason only; never retain subprocess arguments or raw output."""


def error_category(stderr):
    # Match known fragments, never return the original line or surrounding text.
    data = stderr[:8192].lower()
    for fragment, category in (
            (b"read-only file system", "read_only_filesystem"),
            (b"read-only filesystem", "read_only_filesystem"),
            (b"is not running", "container_not_running"),
            (b"no such container", "container_missing"),
            (b"permission denied", "permission_denied"),
            (b"operation not permitted", "operation_not_permitted"),
            (b"no such file or directory", "file_missing"),
            (b"cannot connect to the docker daemon", "daemon_unavailable")):
        if fragment in data:
            return category
    return "command_failed"


class Evidence:
    def __init__(self):
        self.kind = "none"
        self.returncode = None
        self.category = "internal_error"

    def run(self, kind, args, **kwargs):
        self.kind, self.returncode, self.category = kind, None, "internal_error"
        try:
            result = command(args, **kwargs)
        except subprocess.TimeoutExpired:
            self.category = "command_timeout"
            raise SmokeFailure from None
        self.returncode = result.returncode if type(result.returncode) is int and -255 <= result.returncode <= 255 else None
        self.category = error_category(getattr(result, "stderr", b"")) if result.returncode else "unexpected_response"
        return result

    def require_success(self, result):
        if result.returncode != 0:
            raise SmokeFailure

    def payload(self, result):
        try:
            if len(result.stdout) > 4096:
                raise ValueError
            return json.loads(result.stdout)
        except (ValueError, UnicodeError):
            self.category = "invalid_probe_response"
            raise SmokeFailure from None

    def failure(self, name, created):
        # Query only two scalar fields BEFORE teardown; never inspect Env/config,
        # State.Error, healthcheck output, or container logs.
        state, exitcode = "unavailable", None
        if created:
            try:
                result = command(["docker", "inspect", "--format", STATE_FORMAT, name], timeout=5)
                match = re.fullmatch(rb"(created|running|paused|restarting|removing|exited|dead) (-?\d{1,3})\s*", result.stdout[:128])
                if result.returncode == 0 and len(result.stdout) <= 128 and match:
                    state = match[1].decode("ascii")
                    number = int(match[2])
                    exitcode = number if -255 <= number <= 255 else None
            except Exception:
                pass
        return {"cmdkind": self.kind, "returncode": self.returncode,
                "category": self.category, "container_state": state, "container_exitcode": exitcode}


def install_probe(name, probe, evidence):
    assert probe in PROBES
    source = (ROOT / "tests/container" / probe).read_bytes()
    assert 0 < len(source) <= 65536
    result = evidence.run("probe_install", ["docker", "exec", "--interactive", name,
        "python", "-B", "-c", PROBE_WRITER, probe], input=source, timeout=15)
    evidence.require_success(result)


def command(args, **kwargs):
    return subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          timeout=kwargs.pop("timeout", 60), **kwargs)


def runtime_gate(name, evidence):
    for probe in ("supervisor_probe.py", "browser_probe.py"):
        install_probe(name, probe, evidence)
    result = evidence.run("supervisor_capability", ["docker", "exec", name, "python", "-B", "/tmp/supervisor_probe.py",
                      "--capability"], timeout=15)
    if result.returncode == 2 and evidence.payload(result) == {
            "supervisor_probe": "blocked", "stage": "capability"}:
        return "blocked", "supervisor_capability", False
    evidence.require_success(result)
    assert evidence.payload(result) == {
        "supervisor_probe": "passed", "stage": "capability"}
    # The real image's Rust worker launches both fixtures and browser. The observer
    # checks pidfds at ACK receipt, before Docker rm can hide leaked descendants.
    result = evidence.run("supervisor_lifecycle", ["docker", "exec", name, "python", "-B", "/tmp/supervisor_probe.py"], timeout=150)
    evidence.require_success(result)
    assert evidence.payload(result) == {
        "supervisor_probe": "passed", "modes": ["normal", "timeout", "cancel", "termination", "parent_death"],
        "browser_modes": ["normal", "timeout"], "identity": "pidfd", "ack_after_reap": True,
        "before_container_removal": True, "production_worker": True}
    for _ in range(20):
        if evidence.run("readiness", ["docker", "exec", name, "python", "-B", "/app/docker/healthcheck.py"]).returncode == 0:
            return "passed", "done", True
        time.sleep(1)
    return "failed", "readiness", True


def finish_container(name, created, evidence):
    # Lifecycle verification is completed (or failed/blocked) BEFORE this teardown.
    return not created or evidence.run("container_remove", ["docker", "rm", "--force", name]).returncode == 0


def publication_ready(web, capability, descendant_cleanup, cleanup):
    return web and capability == "passed" and descendant_cleanup and cleanup


def main(image):
    name = "anyrouter-smoke-" + secrets.token_hex(8)
    root = secrets.token_urlsafe(48)
    stage = "setup"
    created = False
    cleanup = True
    web = False
    capability = "not_run"
    descendant_cleanup = False
    tested_image_id = ""
    evidence = Evidence()
    failure = None
    try:
        with tempfile.TemporaryDirectory(prefix="anyrouter-container-") as directory:
            config = Path(directory) / "config.toml"
            config.write_text((ROOT / "docker/config.example.toml").read_text().replace(
                "REPLACE_WITH_A_PRIVATE_RANDOM_ROOT_KEY", root))
            config.chmod(0o600)
            if os.getuid() != 1000:
                evidence.require_success(evidence.run("config_owner", ["sudo", "chown", "1000:1000", str(config)]))
            result = evidence.run("image_inspect", ["docker", "image", "inspect", "--format", "{{.Id}}", image])
            tested_image_id = result.stdout.decode().strip()
            assert result.returncode == 0 and re.fullmatch(r"sha256:[0-9a-f]{64}", tested_image_id)
            # Run the immutable ID, not a tag that could be changed after inspection.
            created = True  # Also clean up a partially created/timed-out Docker run.
            result = evidence.run("container_run", ["docker", "run", "--detach", "--name", name,
                "--platform", "linux/amd64", "--network", "none", "--read-only",
                "--cap-drop", "ALL", "--security-opt", "no-new-privileges=true",
                "--mount", f"type=bind,src={config},dst=/config/config.toml,readonly",
                "--tmpfs", "/tmp:rw,nosuid,nodev,size=512m,mode=1777",
                "--tmpfs", "/data:rw,nosuid,nodev,size=16m,uid=1000,gid=1000,mode=0700",
                "--shm-size", "256m", "--memory", "2g", "--pids-limit", "512", tested_image_id])
            assert result.returncode == 0
            stage = "http_probe_install"
            install_probe(name, "http_probe.py", evidence)
            stage = "http_auth"
            for _ in range(20):
                result = evidence.run("http_probe", ["docker", "exec", "--interactive", name, "python", "-B",
                    "/tmp/http_probe.py"], input=(json.dumps({"root": root}) + "\n").encode())
                if result.returncode == 0 and evidence.payload(result) == {"http_probe": "passed"}:
                    break
                time.sleep(1)
            else:
                raise AssertionError
            web = True
            stage = "supervisor_browser_cleanup"
            capability = "failed"  # Exceptions cannot leave the gate looking unattempted.
            capability, stage, descendant_cleanup = runtime_gate(name, evidence)
            if capability == "failed":
                failure = evidence.failure(name, created)
    except Exception:
        failure = evidence.failure(name, created)
    finally:
        if created:
            try:
                cleanup = finish_container(name, created, evidence)
            except Exception:
                cleanup = False
            if not cleanup and failure is None:
                stage = "teardown"
                failure = evidence.failure(name, created)
    ready = publication_ready(web, capability, descendant_cleanup, cleanup)
    blocked = web and capability == "blocked" and cleanup
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a") as stream:
            stream.write(f"publish_ready={'true' if ready else 'false'}\n")
            if ready:
                stream.write(f"tested_image_id={tested_image_id}\n")
    print(json.dumps({"container_smoke": "passed" if ready else "blocked" if blocked else "failed",
                      "web_smoke": "passed" if web else "failed", "capability_gate": capability,
                      "descendant_cleanup": "passed" if descendant_cleanup else "not_passed",
                      "publish_ready": ready, "stage": stage, "cleanup": cleanup,
                      "failure": failure,
                      "offline": True, "synthetic_only": True}))
    return 0 if ready else 2 if blocked else 1


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print('{"container_smoke":"failed","stage":"arguments"}')
        sys.exit(1)
    sys.exit(main(sys.argv[1]))
