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


def command(args, **kwargs):
    return subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          timeout=kwargs.pop("timeout", 60), **kwargs)


def runtime_gate(name):
    for probe in ("supervisor_probe.py", "browser_probe.py"):
        assert command(["docker", "cp", str(ROOT / "tests/container" / probe),
                        name + ":/tmp/" + probe]).returncode == 0
    result = command(["docker", "exec", name, "python", "-B", "/tmp/supervisor_probe.py",
                      "--capability"], timeout=15)
    if result.returncode == 2 and json.loads(result.stdout) == {
            "supervisor_probe": "blocked", "stage": "capability"}:
        return "blocked", "supervisor_capability", False
    assert result.returncode == 0 and json.loads(result.stdout) == {
        "supervisor_probe": "passed", "stage": "capability"}
    # The real image's Rust worker launches both fixtures and browser. The observer
    # checks pidfds at ACK receipt, before Docker rm can hide leaked descendants.
    result = command(["docker", "exec", name, "python", "-B", "/tmp/supervisor_probe.py"], timeout=150)
    assert result.returncode == 0 and json.loads(result.stdout) == {
        "supervisor_probe": "passed", "modes": ["normal", "timeout", "cancel", "termination", "parent_death"],
        "browser_modes": ["normal", "timeout"], "identity": "pidfd", "ack_after_reap": True,
        "before_container_removal": True, "production_worker": True}
    for _ in range(20):
        if command(["docker", "exec", name, "python", "-B", "/app/docker/healthcheck.py"]).returncode == 0:
            return "passed", "done", True
        time.sleep(1)
    return "failed", "readiness", True


def finish_container(name, created):
    # Lifecycle verification is completed (or failed/blocked) BEFORE this teardown.
    return not created or command(["docker", "rm", "--force", name]).returncode == 0


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
    try:
        with tempfile.TemporaryDirectory(prefix="anyrouter-container-") as directory:
            config = Path(directory) / "config.toml"
            config.write_text((ROOT / "docker/config.example.toml").read_text().replace(
                "REPLACE_WITH_A_PRIVATE_RANDOM_ROOT_KEY", root))
            config.chmod(0o600)
            if os.getuid() != 1000:
                assert command(["sudo", "chown", "1000:1000", str(config)]).returncode == 0
            result = command(["docker", "image", "inspect", "--format", "{{.Id}}", image])
            tested_image_id = result.stdout.decode().strip()
            assert result.returncode == 0 and re.fullmatch(r"sha256:[0-9a-f]{64}", tested_image_id)
            # Run the immutable ID, not a tag that could be changed after inspection.
            created = True  # Also clean up a partially created/timed-out Docker run.
            result = command(["docker", "run", "--detach", "--name", name,
                "--platform", "linux/amd64", "--network", "none", "--read-only",
                "--cap-drop", "ALL", "--security-opt", "no-new-privileges=true",
                "--mount", f"type=bind,src={config},dst=/config/config.toml,readonly",
                "--tmpfs", "/tmp:rw,nosuid,nodev,size=512m,mode=1777",
                "--tmpfs", "/data:rw,nosuid,nodev,size=16m,uid=1000,gid=1000,mode=0700",
                "--shm-size", "256m", "--memory", "2g", "--pids-limit", "512", tested_image_id])
            assert result.returncode == 0
            stage = "http_auth"
            assert command(["docker", "cp", str(ROOT / "tests/container/http_probe.py"),
                            name + ":/tmp/http_probe.py"]).returncode == 0
            for _ in range(20):
                result = command(["docker", "exec", "--interactive", name, "python", "-B",
                    "/tmp/http_probe.py"], input=(json.dumps({"root": root}) + "\n").encode())
                if result.returncode == 0 and json.loads(result.stdout) == {"http_probe": "passed"}:
                    break
                time.sleep(1)
            else:
                raise AssertionError
            web = True
            stage = "supervisor_browser_cleanup"
            capability = "failed"  # Exceptions cannot leave the gate looking unattempted.
            capability, stage, descendant_cleanup = runtime_gate(name)
    except Exception:
        pass
    finally:
        if created:
            try:
                cleanup = finish_container(name, created)
            except Exception:
                cleanup = False
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
                      "offline": True, "synthetic_only": True}))
    return 0 if ready else 2 if blocked else 1


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print('{"container_smoke":"failed","stage":"arguments"}')
        sys.exit(1)
    sys.exit(main(sys.argv[1]))
