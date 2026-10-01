"""Mock/static evidence only. Does not claim an image or runtime was tested."""
import ast
import base64
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import tomllib
import tempfile
from contextlib import redirect_stdout
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def main():
    checks = 0
    docker = (ROOT / "Dockerfile").read_text()
    workflow = (ROOT / ".github/workflows/container.yml").read_text()
    config = tomllib.loads((ROOT / "docker/config.example.toml").read_text())
    assert all(base in docker for base in (
        "FROM node:24-bookworm-slim", "FROM rust:bookworm",
        "FROM python:3.13-slim-bookworm"))
    assert "npm ci" in docker and "npm test && npm run typecheck && npm run build" in docker
    assert "cargo build --locked --release" in docker and "uv sync --frozen --no-dev" in docker
    assert "uv==0.11.3" in docker and "UV_PYTHON_DOWNLOADS=never" in docker
    checks += 1

    helper = tomllib.loads((ROOT / "tools/browser-helper/pyproject.toml").read_text())
    assert helper["project"]["dependencies"] == ["cloakbrowser==0.5.11", "playwright==1.58.0"]
    assert helper["tool"]["uv"]["package"] is False
    checks += 1

    nix = (ROOT / "nix/cloak-browser.nix").read_text()
    assert "146.0.7680.177.5" in nix and "146.0.7680.177.5" in docker
    digest = base64.b64decode("ShK83pX6G7G+7ytBq15cJ8Nr544749DayMZNcFIWZw4=", validate=True).hex()
    assert digest in docker and "sha256sum --check --strict" in docker
    assert 'test "$TARGETARCH" = amd64' in docker
    assert "COPY --from=browser /opt/cloakbrowser/" in docker
    checks += 1

    assert "USER 1000:1000" in docker and "EXPOSE 8080" in docker
    assert '"/usr/bin/tini", "--"' in docker
    assert "COPY LICENSE THIRD_PARTY_NOTICES.md" in docker
    assert "PROJECT-BINARY-LICENSE.md" in docker and "CLOAKBROWSER_AUTO_UPDATE=false" in docker
    assert "PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1" in docker
    assert "root_key" not in docker and "playwright install" not in docker
    assert "0700" in (ROOT / "docker/entrypoint.sh").read_text()
    checks += 1

    assert config["container_mode"] is True and config["cookie_secure"] is False
    assert config["bind"] == "0.0.0.0:8080" and config["state_path"] == "/data/private/account.json"
    assert config["browser_helper_executable"] == "/app/tools/browser-helper/.venv/bin/python"
    assert config["allowed_origin"] == ["http://localhost:8080", "http://127.0.0.1:8080"]
    assert "REPLACE" in config["root_key"]  # deliberately invalid, not a deployable secret
    checks += 1

    pins = dict(re.findall(r"uses: ([\w/-]+)@([0-9a-f]{40})", workflow))
    assert pins == {
        "actions/checkout": "d23441a48e516b6c34aea4fa41551a30e30af803",
        "docker/login-action": "dbcb813823bdd20940b903addbd779551569679f",
        "docker/setup-buildx-action": "f87e5991a6d7451dcb8d9637bfbc97413f497069",
        "docker/build-push-action": "c3c9e263c25d99ce0380d002d59b67737d91b0dc",
    }
    validate, publish = workflow.split("  publish:", 1)
    assert "github.event.repository.private == true" in validate and "github.event.repository.private == true" in publish
    assert "packages: write" not in validate and "docker/login-action" not in validate
    assert "needs: validate" in publish and "refs/heads/main" in publish
    assert "github.event.repository.default_branch" in publish
    assert "github.event_name == 'push'" in publish and "github.event_name == 'workflow_dispatch'" in publish
    assert '"$IMAGE:sha-$REVISION"' in publish and "REVISION: ${{ github.sha }}" in publish
    assert "linux/amd64" in validate and "${REPOSITORY,,}" in publish
    assert publish.count("uses: docker/build-push-action@") == 1
    assert publish.index("Build and load once") < publish.index("Test exact loaded image") < publish.index("docker push")
    assert 'docker tag "$TESTED_IMAGE_ID"' in publish and "tested_image_id" in publish
    assert "continue-on-error" not in workflow
    assert "Build-only evidence" in workflow and "No login or publication" in workflow
    for step in publish.split("      - ")[1:]:
        if any(term in step for term in ("Lowercase private", "GHCR package", "docker/login-action", "Tag and push", "Verify published")):
            assert "if: steps.smoke.outputs.publish_ready == 'true'" in step
    assert "DOCKER_BUILD_RECORD_UPLOAD: false" in workflow
    assert "Refuse an existing non-private GHCR package" in publish
    assert "docker/ghcr_visibility.py --require-existing" in publish
    checks += 1
    rust = (ROOT / "docker/rust-tests.sh").read_text()
    assert "RUN /bin/sh /build/rust-tests.sh" in docker
    assert "cargo test --locked" in rust and "--exact" in rust
    assert "1 passed; 0 failed; 0 ignored;" in rust and "NOT passed" in rust
    expected = {
        "config::tests::login_diagnostics_is_explicit_and_backward_compatible": "config.rs",
        "config::tests::container_origins_and_executable_are_explicit": "config.rs",
        "diagnostics::tests::exact_schema_roundtrip_and_strict_bounds": "diagnostics.rs",
        "error::tests::source_metadata_is_finite_and_absent_by_default": "error.rs",
        "app::runtime_tests::secure_cookie_has_same_creation_deletion_policy": "app.rs",
        "helper::container_command_is_explicit_and_native_command_keeps_namespace": "helper.rs",
        "tests::cookie_structure_requires_finite_expiry_but_not_live_session": "tests.rs",
        "tests::cookie_selection_obeys_domain_secure_expiry_path_and_precedence": "tests.rs",
        "upstream_body::tests::gzip_is_bounded_and_requires_complete_valid_stream": "upstream_body.rs",
        "log_settings::tests::strict_defaults_thresholds_and_fixed_errors": "log_settings.rs",
        "log_settings::tests::update_commit_hot_watch_failure_keeps_old_and_reopen": "log_settings.rs",
        "helper::tests::session_payload_prunes_expired_and_canonicalizes_session_expiry": "helper.rs",
        "helper::tests::session_payload_expired_is_not_malformed_but_unsafe_fields_still_are": "helper.rs",
    }
    selected = re.findall(r"^    ([a-zA-Z_][\w:]*)[ \t]*\\?$", rust, re.M)
    assert len(selected) == len(expected) == 13 and set(selected) == set(expected)
    assert f"# Execute {len(expected)} reviewed pure tests;" in docker
    for name, source in expected.items():
        assert name in rust
        assert f"fn {name.rsplit('::', 1)[1]}()" in (ROOT / "backend/src" / source).read_text()
    checks += 1

    smoke = (ROOT / "tests/container/ci_smoke.py").read_text()
    assert '"--network", "none"' in smoke and '"--read-only"' in smoke
    assert '"--cap-drop", "ALL"' in smoke and '"no-new-privileges=true"' in smoke
    assert "unshare" not in smoke and "namespace_probe.py" not in smoke
    assert "unconfined" not in smoke and "--privileged" not in smoke
    assert "config.chmod(0o600)" in smoke and "TemporaryDirectory" in smoke
    assert '"docker", "rm", "--force", name' in smoke
    assert "about:blank" in (ROOT / "tests/container/browser_probe.py").read_text()
    http = (ROOT / "tests/container/http_probe.py").read_text()
    assert "/api/account/login" not in http and "/v1/" not in http
    assert "json.loads(sys.stdin.buffer.readline" in http
    checks += 1

    ignore = (ROOT / ".dockerignore").read_text().splitlines()
    assert all(pattern in ignore for pattern in (".git", ".slim/", ".env*", "**/config.toml", "**/data",
        "**/target", "**/node_modules", "**/dist", "**/.venv", "**/*.log", "**/traces"))
    assert not any("lock" in line for line in ignore if not line.startswith("flake"))
    for path in [*ROOT.glob("docker/*.py"), *ROOT.glob("tests/container/*.py")]:
        ast.parse(path.read_text(), filename=str(path))
    checks += 1

    spec = importlib.util.spec_from_file_location("container_healthcheck", ROOT / "docker/healthcheck.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    with patch.object(module.http.client, "HTTPConnection") as connection:
        response = connection.return_value.getresponse.return_value
        response.status = 200
        response.getheader.return_value = "application/json"
        assert module.ready()
        response.status = 503
        assert not module.ready()
        response.status = 200
        response.getheader.return_value = "text/html"
        assert not module.ready()
        connection.return_value.request.side_effect = RuntimeError("SENTINEL-private-exception")
        assert not module.ready()
        assert connection.return_value.close.called
    checks += 1
    spec = importlib.util.spec_from_file_location("ghcr_visibility", ROOT / "docker/ghcr_visibility.py")
    visibility = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(visibility)
    env = {"REPOSITORY": "FixtureOwner/FixtureRepo", "OWNER_TYPE": "User", "GH_TOKEN": "SENTINEL-CI-TOKEN"}
    with patch.dict(visibility.os.environ, env), patch.object(visibility, "urlopen") as fetch:
        response = fetch.return_value.__enter__.return_value
        response.read.return_value = b'{"visibility":"private"}'
        assert visibility.private_or_new(True)
        response.read.return_value = b'{"visibility":"public"}'
        assert not visibility.private_or_new()
        response.read.return_value = b'{"visibility":"internal"}'
        assert not visibility.private_or_new()
        fetch.side_effect = visibility.HTTPError("https://fixture.invalid", 404, "SENTINEL", {}, None)
        assert visibility.private_or_new() and not visibility.private_or_new(True)
        fetch.side_effect = visibility.HTTPError("https://fixture.invalid", 403, "SENTINEL", {}, None)
        assert not visibility.private_or_new()
    checks += 1
    spec = importlib.util.spec_from_file_location("container_smoke", ROOT / "tests/container/ci_smoke.py")
    smoke_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(smoke_module)
    # Full orchestration mocked: neither Docker nor a worker is executed here.
    for scenario in ("passed", "denied", "leaked", "browser_failed", "readiness_failed", "teardown_failed"):
        calls = []

        def fake_command(args, **kwargs):
            calls.append(args)
            code, body = 0, b""
            if args[1:3] == ["image", "inspect"]:
                body = ("sha256:" + "a" * 64 + "\n").encode()
            elif args[1] == "exec" and args[-1] == "/tmp/http_probe.py":
                body = b'{"http_probe":"passed"}'
            elif args[1] == "exec" and args[-1] == "--capability":
                code = 2 if scenario == "denied" else 0
                body = json.dumps({"supervisor_probe": "blocked" if code else "passed", "stage": "capability"}).encode()
            elif args[1] == "exec" and args[-1] == "/tmp/supervisor_probe.py":
                code = 1 if scenario in ("leaked", "browser_failed") else 0
                body = json.dumps({"supervisor_probe": "passed",
                    "modes": ["normal", "timeout", "cancel", "termination", "parent_death"],
                    "browser_modes": ["normal", "timeout"], "identity": "pidfd", "ack_after_reap": True,
                    "before_container_removal": True, "production_worker": True}).encode()
            elif args[1] == "exec" and args[-1] == "/app/docker/healthcheck.py":
                code = 1 if scenario == "readiness_failed" else 0
            elif args[1] == "rm":
                code = 1 if scenario == "teardown_failed" else 0
            return SimpleNamespace(returncode=code, stdout=body)

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "outputs"
            with patch.object(smoke_module, "command", fake_command), patch.object(smoke_module.os, "getuid", return_value=1000), \
                    patch.object(smoke_module.time, "sleep"), patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}), redirect_stdout(io.StringIO()) as printed:
                status = smoke_module.main("fixture:ci")
            report = json.loads(printed.getvalue())
            assert status == (0 if scenario == "passed" else 2 if scenario == "denied" else 1)
            assert report["web_smoke"] == "passed" and report["publish_ready"] == (scenario == "passed")
            assert ("publish_ready=true" in output.read_text()) == (scenario == "passed")
            assert ("tested_image_id=" in output.read_text()) == (scenario == "passed")
        assert calls[-1][1] == "rm"
        if scenario == "denied":
            assert report["capability_gate"] == "blocked" and report["descendant_cleanup"] == "not_passed"
            assert not any(args[1] == "exec" and args[-1] == "/tmp/supervisor_probe.py" for args in calls)
        else:
            probe = next(i for i, args in enumerate(calls) if args[1] == "exec" and args[-1] == "/tmp/supervisor_probe.py")
            assert probe < len(calls) - 1
        assert next(args for args in calls if args[1] == "run")[-1] == "sha256:" + "a" * 64
    for flags in ((False, "passed", True, True), (True, "blocked", True, True), (True, "passed", False, True), (True, "passed", True, False)):
        assert not smoke_module.publication_ready(*flags)
    checks += 1

    spec = importlib.util.spec_from_file_location("supervisor_probe", ROOT / "tests/container/supervisor_probe.py")
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    owned = [(42, "fixture"), (43, "crashpad")]
    for ack, gone, passed in ((b"\x01", True, True), (b"\x00", True, False),
                               (b"", True, False), (b"\x01\x01", True, False), (b"\x01", False, False)):
        with patch.object(native, "wait_until") as waited, patch.object(native.os, "read", return_value=ack), \
                patch.object(native, "reaped", return_value=gone):
            try:
                native.accept_ack(99, owned, b"\x01")
            except AssertionError:
                accepted = False
            else:
                accepted = True
            assert accepted == passed
            waited.assert_called_once()
    with patch.object(native, "wait_until"), patch.object(native.os, "read", return_value=b"\x00"), \
            patch.object(native, "reaped", return_value=True):
        native.accept_ack(99, owned, b"\x00")  # cancellation ACK is valid, not helper success
    # POLLIN means exited but not necessarily reaped; require POLLHUP.
    with patch.object(native.select, "poll") as poll:
        poll.return_value.poll.return_value = [(42, native.select.POLLIN)]
        assert not native.reaped(42)
        poll.return_value.poll.return_value = [(42, native.select.POLLHUP)]
        assert native.reaped(42)
    checks += 1

    from unittest.mock import Mock
    # Exercise cleanup ordering without spawning/forking/killing any real process.
    for mode in ("normal", "timeout", "cancel", "termination"):
        events = []
        child = Mock(pid=100)
        child.stdout.read.return_value = b'{}'
        child.wait.side_effect = lambda **kwargs: events.append("wait") or 0
        with patch.object(native, "start_worker", return_value=(child, 80, 81)), \
                patch.object(native, "wait_until"), patch.object(native, "descendants", return_value=owned), \
                patch.object(native, "accept_ack", side_effect=lambda *args: events.append("ack_reaped")), \
                patch.object(native.os, "pidfd_open", return_value=82), patch.object(native.os, "close"), \
                patch.object(native.select, "select", return_value=([], [], [])), \
                patch.object(native.signal, "pidfd_send_signal") as kill:
            native.lifecycle(mode)
            assert events.index("ack_reaped") < events.index("wait")
            if mode == "termination":
                kill.assert_called_once_with(82, native.signal.SIGTERM)
            else:
                kill.assert_not_called()
    checks += 1

    # Verify actual FD/argv adapter without starting Rust or creating real pipes.
    with patch.object(native.os, "pipe2", side_effect=[(70, 71), (72, 73)]), \
            patch.object(native.os, "getpid", return_value=60), patch.object(native.os, "close") as close, \
            patch.object(native.subprocess, "Popen") as spawn:
        child, control, ack = native.start_worker([native.PYTHON, "-B", "fixture.py"], "/tmp")
        args, kwargs = spawn.call_args
        assert args[0] == [native.BACKEND, native.WORKER, "60", "70", "73", native.PYTHON, "-B", "fixture.py"]
        assert kwargs["pass_fds"] == (70, 73) and kwargs["stderr"] == native.subprocess.DEVNULL
        assert (control, ack) == (71, 72)
        child.stdin.write.assert_called_once_with(b"{}\n")
        child.stdin.close.assert_called_once()
        assert [call.args[0] for call in close.call_args_list] == [70, 73]
    checks += 1

    # Parent SIGKILL must be independent of EOF: transferred control stays open
    # through ACK. SCM_RIGHTS keeps the outer observer outside the job tree.
    events = []
    parent = Mock(pid=100)
    parent.wait.side_effect = lambda **kwargs: events.append("parent_wait") or -native.signal.SIGKILL
    parent.poll.return_value = -native.signal.SIGKILL
    fds = native.array.array("i", [80, 81, 83])
    with patch.object(native.socket, "socketpair") as pair, patch.object(native.subprocess, "Popen", return_value=parent), \
            patch.object(native, "wait_until"), patch.object(native, "descendants", return_value=owned), \
            patch.object(native, "accept_ack", side_effect=lambda *args: events.append("ack_reaped")), \
            patch.object(native.os, "pidfd_open", side_effect=[84, 82]), \
            patch.object(native.os, "close", side_effect=lambda fd: events.append(("close", fd))), \
            patch.object(native.signal, "pidfd_send_signal") as kill:
        from unittest.mock import MagicMock
        outer, inner = MagicMock(), MagicMock()
        pair.return_value = outer, inner
        inner.fileno.return_value = 85
        outer.recvmsg.return_value = (b"101", [(native.socket.SOL_SOCKET, native.socket.SCM_RIGHTS, fds.tobytes())], 0, None)
        native.lifecycle("parent_death")
        kill.assert_called_once_with(84, native.signal.SIGKILL)
        assert events.index("parent_wait") < events.index("ack_reaped") < events.index(("close", 80))
    checks += 1

    # Capability denial and a failed lifecycle cannot be converted into a pass.
    for available, failure in ((False, False), (True, True), (True, False)):
        with patch.object(native, "capability", return_value=available), \
                patch.object(native, "lifecycle", side_effect=AssertionError("SENTINEL") if failure else None) as lifecycle, \
                patch.object(native.sys, "argv", ["probe"]), redirect_stdout(io.StringIO()) as printed:
            status = native.main()
        assert status == (2 if not available else 1 if failure else 0)
        assert "SENTINEL" not in printed.getvalue()
        assert lifecycle.call_count == (0 if not available else 1 if failure else 7)
    checks += 1

    # Pin the adapter to the ACTUAL worker interface, not an invented smoke API.
    supervisor = (ROOT / "backend/src/supervisor.rs").read_text()
    probe = (ROOT / "tests/container/supervisor_probe.py").read_text()
    main_rs = (ROOT / "backend/src/main.rs").read_text()
    helper_rs = (ROOT / "backend/src/helper.rs").read_text()
    assert 'const WORKER: &str = "--internal-browser-worker"' in supervisor
    assert all(term in supervisor for term in ("let parent = number(0)?", "let control_fd = number(1)?", "let ack_fd = number(2)?", 'args[3] == "--probe"'))
    assert supervisor.index("cleanup()?;") < supervisor.index("ack.write_all(&[u8::from(success)])?")
    assert "supervisor::dispatch();" in main_rs and "supervisor::available().await" in main_rs
    assert "if self.container_mode" in helper_rs and "crate::supervisor::spawn" in helper_rs
    assert '"--probe"' in probe and "SCM_RIGHTS" in probe and "parent.wait(timeout=5) == -signal.SIGKILL" in probe
    assert "killpg" not in probe and "unshare" not in probe and "getuid" not in probe
    assert "stdout=subprocess.PIPE, stderr=subprocess.DEVNULL" in probe
    browser_probe = (ROOT / "tests/container/browser_probe.py").read_text()
    assert 'await context.route("**/*"' in browser_probe and 'page.goto("about:blank")' in browser_probe
    assert '/tmp/browser_probe.py' in probe and 'start_worker(helper, directory)' in probe
    assert '"/tmp/browser_probe.py"' not in smoke  # never docker-exec Python browser directly
    checks += 1
    print(json.dumps({"container_contract": "passed", "checks": checks, "mock_static_only": True}))


if __name__ == "__main__":
    main()
