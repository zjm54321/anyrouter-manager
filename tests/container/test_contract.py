"""Mock/static evidence only. Does not claim an image or runtime was tested."""
import ast
import asyncio
import base64
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import runpy
import sys
import tomllib
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def browser_notice_wiring():
    """Execute the real adapter with mocks, not a browser/runtime pass."""
    from unittest.mock import AsyncMock, Mock
    spec = importlib.util.spec_from_file_location("browser_probe", ROOT / "tests/container/browser_probe.py")
    adapter = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(adapter)
    spec = importlib.util.spec_from_file_location("local_browser_smoke", ROOT / "tools/browser-helper/tests/local_browser_smoke.py")
    fixture_module = importlib.util.module_from_spec(spec)
    with patch.object(sys, 'path', sys.path.copy()):
        spec.loader.exec_module(fixture_module)
    for scenario in ("normal", "timeout", "fixture_failure", "wrong_count", "render_failure",
                      "navigation_failure", "navigation_wrong_count", "self_failure", "self_wrong_count"):
        page = SimpleNamespace(goto=AsyncMock(), evaluate=AsyncMock(return_value=2),
                               route=AsyncMock(), unroute=AsyncMock())
        context = SimpleNamespace(new_page=AsyncMock(return_value=page), route=AsyncMock(), close=AsyncMock())
        browser = SimpleNamespace(new_context=AsyncMock(return_value=context), close=AsyncMock())
        launch = AsyncMock(return_value=browser)
        launcher = Mock(return_value=launch)
        finish = Mock()
        finish.exists.side_effect = [False, True]
        ready = Mock()
        sleep = AsyncMock(side_effect=RuntimeError("intentional_timeout") if scenario == "timeout" else None)

        async def fixture(actual_page, report, render):
            assert actual_page is page
            if scenario == "fixture_failure":
                raise AssertionError("fixture_failed")
            if scenario == "render_failure":
                page.goto.side_effect = RuntimeError("render_failed")
            await render("<h5>synthetic fixture</h5>")
            url, serve = page.route.await_args.args
            assert url == "https://anyrouter.top/login"
            route = SimpleNamespace(fulfill=AsyncMock())
            await serve(route)
            route.fulfill.assert_awaited_once_with(status=200, content_type="text/html; charset=utf-8",
                                                  body="<h5>synthetic fixture</h5>")
            page.unroute.assert_awaited_once_with(url, serve)
            report["native_notice_backdrop_cases"] = 9 if scenario == "wrong_count" else 10

        invoke = AsyncMock(side_effect=fixture)

        async def navigation_fixture(actual_page, report):
            assert actual_page is page and report == {"native_notice_backdrop_cases": 10}
            if scenario == "navigation_failure":
                raise AssertionError("navigation_fixture_failed")
            report["native_navigation_readiness_cases"] = 0 if scenario == "navigation_wrong_count" else 1

        navigation = AsyncMock(side_effect=navigation_fixture)

        async def self_fixture(actual_browser, actual_context, actual_page, report):
            assert (actual_browser, actual_context, actual_page) == (browser, context, page)
            if scenario == "self_failure":
                raise fixture_module.ProactiveSelfFailure('helper_result', 'user_self_unverified')
            report["native_proactive_self_cases"] = 0 if scenario == "self_wrong_count" else 1

        proactive = AsyncMock(side_effect=self_fixture)
        modules = {"browser_helper.core": SimpleNamespace(browser_launcher=launcher),
                     "local_browser_smoke": SimpleNamespace(notice_backdrop_cases=invoke,
                                                            navigation_readiness_case=navigation,
                                                            proactive_self_case=proactive,
                                                            ProactiveSelfFailure=fixture_module.ProactiveSelfFailure)}
        with patch.dict("sys.modules", modules), patch.object(adapter.sys, "path", adapter.sys.path.copy()), \
                patch.object(adapter.sys, "stdin", SimpleNamespace(buffer=io.BytesIO(b"{}\n"))), \
                patch.object(adapter, "Path", side_effect=lambda name: ready if name == "ready" else finish), \
                patch.object(adapter.asyncio, "sleep", sleep), redirect_stdout(io.StringIO()) as printed:
            status = asyncio.run(adapter.main())
            assert status == (0 if scenario == "normal" else 1)
        launcher.assert_called_once_with()
        launch.assert_awaited_once_with(headless=True)
        ready.write_text.assert_called_once()
        sleep.assert_awaited_once_with(.01)
        context.route.assert_awaited_once()
        assert context.route.await_args.args[0] == "**/*"
        assert invoke.await_count == (0 if scenario == "timeout" else 1)
        assert navigation.await_count == (0 if scenario in ("timeout", "fixture_failure", "render_failure") else 1)
        assert proactive.await_count == (0 if scenario in ("timeout", "fixture_failure", "render_failure", "navigation_failure", "wrong_count") else 1)
        if scenario == "normal":
            assert json.loads(printed.getvalue()) == {
                "fixture": "done", "notice_backdrop_cases": 10, "notice_backdrop": "passed",
                "navigation_readiness_cases": 1, "proactive_self_cases": 1}
            assert page.goto.await_args.args == ("about:blank",)
            assert page.evaluate.await_count == 2
            context.close.assert_awaited_once()
            browser.close.assert_awaited_once()
        else:
            failure = json.loads(printed.getvalue())
            assert adapter.browser_failure(failure) == failure
            assert failure['phase'] == {
                'timeout': 'ready_wait', 'fixture_failure': 'notice', 'render_failure': 'notice',
                'wrong_count': 'navigation', 'navigation_failure': 'navigation',
                'navigation_wrong_count': 'final_check', 'self_failure': 'proactive_self',
                'self_wrong_count': 'final_check'}[scenario]
            assert failure['category'] == ('unexpected' if scenario in ('timeout', 'render_failure') else 'assertion')
            if scenario == 'self_failure':
                assert failure['assertion_kind'] == 'helper_result' and failure['helper_error'] == 'user_self_unverified'
        if scenario == "render_failure":
            page.unroute.assert_awaited_once()  # Synthetic route cannot survive render failure.
    return fixture_module.ProactiveSelfFailure


def diagnostic_envelopes(smoke, native, proactive_type):
    """Real projection boundaries, mocked lifecycle; no browser or worker runs."""
    from unittest.mock import Mock
    import browser_probe as browser

    sentinel = 'SENTINEL-private-body-cookie-url'
    for exc, category in ((TimeoutError(sentinel), 'timeout'), (AssertionError(sentinel), 'assertion'),
                          (ImportError(sentinel), 'import'), (OSError(sentinel), 'os_error'),
                          (RuntimeError(sentinel), 'unexpected')):
        detail = browser.exception_report('notice', exc)
        assert detail == {'fixture': 'failed', 'phase': 'notice', 'category': category}
    with patch.object(browser, 'run', side_effect=ImportError(sentinel)), redirect_stdout(io.StringIO()) as printed:
        assert asyncio.run(browser.main()) == 1
    assert json.loads(printed.getvalue()) == browser.exception_report('launch', ImportError())
    for phase in browser.PHASES:
        assert browser.browser_failure(browser.exception_report(phase, AssertionError())) is not None
    with patch.object(browser, 'run', side_effect=asyncio.CancelledError), redirect_stdout(io.StringIO()) as printed:
        try:
            asyncio.run(browser.main())
        except asyncio.CancelledError:
            pass
        else:
            raise AssertionError('cancellation swallowed')
        assert printed.getvalue() == ''
    for kind in browser.ASSERTIONS:
        for error in (*browser.HELPER_ERRORS, sentinel):
            detail = browser.exception_report('proactive_self', proactive_type(kind, error), proactive_type)
            assert detail['assertion_kind'] == kind and sentinel not in json.dumps(detail)
            assert ('helper_error' in detail) == (kind == 'helper_result' and error != sentinel)
            assert len(json.dumps(detail).encode()) < 4096
    detail = browser.exception_report('proactive_self', proactive_type('helper_result', 'user_self_unverified'), proactive_type)
    # Distinguish failed JSON observation from an explicit negative login while
    # forwarding only the approved finite subset through both wire boundaries.
    from browser_helper import core
    for login_json, login_success in ((False, None), (True, False)):
        state = {'phase': 'profile_wait', 'page': 'console', 'login_requested': True,
                 'login_status': 200, 'login_json': login_json, 'login_success': login_success,
                 'body': sentinel, 'cookie': sentinel, 'url': sentinel}
        exc = proactive_type('helper_result', 'login_failed', core.diagnostics(state))
        detail = browser.exception_report('proactive_self', exc, proactive_type)
        diagnostics = detail['diagnostics']
        assert diagnostics['login_json'] is login_json and diagnostics['login_success'] is login_success
        wire = json.dumps({'supervisor_probe': 'failed', 'stage': 'browser_normal', 'browser_failure': detail}).encode()
        assert len(wire) < 4096 and sentinel.encode() not in wire
        assert smoke.supervisor_failure(wire)['browser_failure'] == detail
        for invalid in (diagnostics | {'body': sentinel}, diagnostics | {'phase': sentinel},
                        diagnostics | {'login_json': 1}, diagnostics | {'login_success': 'false'},
                        diagnostics | {'login_status': True}, diagnostics | {'self_status': 600}):
            rejected = detail | {'diagnostics': invalid}
            assert browser.browser_failure(rejected) is None
            assert smoke.supervisor_failure(json.dumps({'supervisor_probe': 'failed', 'stage': 'browser_normal',
                                                       'browser_failure': rejected}).encode()) is None
        assert browser.browser_failure(detail | {'assertion_kind': 'returned_cookie'}) is None
    # Keep diagnostics on the detail used below: real pipe, supervisor main and
    # final smoke evidence must all preserve it without turning failure to pass.
    envelope = {'supervisor_probe': 'failed', 'stage': 'browser_normal', 'browser_failure': detail}
    for stage in ('capability', *(f'synthetic_{mode}' for mode in native.MODES), 'browser_normal', 'browser_timeout'):
        assert smoke.supervisor_failure(json.dumps({'supervisor_probe': 'blocked' if stage == 'capability' else 'failed',
                                                   'stage': stage}).encode()) == {'stage': stage}
    for invalid in (b'', b'not-json-' + sentinel.encode(), b'[]', b'x' * 4097,
                    json.dumps(envelope | {'secret': sentinel}).encode(),
                    json.dumps(envelope | {'stage': sentinel}).encode(),
                    json.dumps(envelope | {'browser_failure': detail | {'secret': sentinel}}).encode(),
                    json.dumps(envelope | {'browser_failure': detail | {'helper_error': sentinel}}).encode(),
                    json.dumps(envelope | {'browser_failure': detail | {'category': []}}).encode()):
        assert smoke.supervisor_failure(invalid) is None
    assert browser.browser_failure(detail | {'phase': sentinel}) is None

    # Real bounded nonblocking pipe reader: even an exited worker may leave a
    # live inherited writer. Missing EOF must not hang or yield trusted evidence.
    for body, running, writer_open in ((json.dumps(detail).encode(), False, False),
                                      (json.dumps(detail).encode(), True, False),
                                      (json.dumps(detail).encode(), False, True),
                                      (b'x' * 4097, False, False),
                                      (json.dumps(detail | {'secret': sentinel}).encode(), False, False)):
        reader, writer = os.pipe()
        stream = os.fdopen(reader, 'rb')
        try:
            os.write(writer, body)
            if not writer_open:
                os.close(writer)
            child = SimpleNamespace(poll=lambda: None if running else 0, stdout=stream)
            result = native.exited_browser_failure(child)
            assert result == (detail if not running and not writer_open and body == json.dumps(detail).encode() else None)
        finally:
            stream.close()
            if writer_open:
                os.close(writer)

    # Failure -> existing cancel/wait/FD cleanup -> diagnostic read, not ACK proof.
    real_close = os.close
    for cleanup_fails in (False, True):
        events = []
        original = AssertionError(sentinel)
        child = Mock(pid=100)
        child.wait.side_effect = lambda **kw: events.append('wait') or 0
        def close(fd):
            if fd not in (80, 81, 82, 142, 143, 144, 145):
                return real_close(fd)
            events.append(('close', fd))
            if cleanup_fails:
                raise OSError(sentinel)
        with patch.object(native, 'start_worker', return_value=(child, 80, 81)), \
                patch.object(native, 'wait_until'), patch.object(native, 'descendants', return_value=[(i, 'crashpad') for i in range(142, 146)]), \
                patch.object(native, 'accept_ack', side_effect=original), patch.object(native.os, 'pidfd_open', return_value=82), \
                patch.object(native.os, 'close', side_effect=close), \
                patch.object(native, 'exited_browser_failure', side_effect=lambda child: events.append('diagnostic') or detail):
            try:
                native.lifecycle('normal', browser=True)
            except native.LifecycleFailure as exc:
                assert exc.__cause__ is original and exc.detail == detail
            else:
                raise AssertionError('failure became success')
        assert events.index(('close', 80)) < events.index('diagnostic')
        if not cleanup_fails:
            assert events.index('wait') < events.index(('close', 82)) < events.index('diagnostic')
        child.stdout.close.assert_called_once()

    # Actual supervisor main -> bounded outer decoder -> safe final evidence.
    def lifecycle(mode, browser=False):
        if browser:
            raise native.LifecycleFailure(detail)
    with patch.object(native, 'capability', return_value=True), patch.object(native, 'lifecycle', lifecycle), \
            patch.object(native.sys, 'argv', ['probe']), redirect_stdout(io.StringIO()) as printed:
        assert native.main() == 1
    assert json.loads(printed.getvalue()) == envelope
    evidence = smoke.Evidence()
    results = [SimpleNamespace(returncode=0, stdout=b'{"supervisor_probe":"passed","stage":"capability"}', stderr=b''),
               SimpleNamespace(returncode=1, stdout=printed.getvalue().encode(), stderr=sentinel.encode())]
    with patch.object(smoke, 'install_probe'), patch.object(smoke, 'command', side_effect=results):
        try:
            smoke.runtime_gate('fixture', evidence)
        except smoke.SmokeFailure:
            pass
        else:
            raise AssertionError('failed lifecycle accepted')
    failure = evidence.failure('fixture', False)
    assert failure['probe_failure'] == {'stage': 'browser_normal', 'browser_failure': detail}
    assert sentinel not in json.dumps(failure)


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
    missing_copy_sources = [source for source in ("LICENSE", "THIRD_PARTY_NOTICES.md")
                            if not (ROOT / source).is_file()]
    assert not missing_copy_sources, "Dockerfile COPY source missing: " + ", ".join(missing_copy_sources)
    assert 'org.opencontainers.image.licenses="GPL-3.0-or-later"' in docker
    assert 'dev.anyrouter.browser-license="CloakBrowser Binary License v1.0; separate redistribution permission required;' in docker
    assert "PROJECT-WRAPPER-LICENSE" in docker and "internal-use image only" not in docker
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
    assert "github.event.repository.private" not in validate and "github.event.repository.private == false" in publish
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
    for step in publish.split("      - ")[1:]:
        if any(term in step for term in ("Lowercase public", "GHCR package", "docker/login-action", "Tag and push", "Verify published")):
            assert "if: steps.smoke.outputs.publish_ready == 'true'" in step
    assert "DOCKER_BUILD_RECORD_UPLOAD: false" in workflow
    assert "Require an existing public GHCR package before login" in publish
    assert publish.count("run: python3 -B docker/ghcr_visibility.py\n") == 2
    checks += 1
    rust = (ROOT / "docker/rust-tests.sh").read_text()
    assert "RUN /bin/sh /build/rust-tests.sh" in docker
    assert "cargo test --locked" in rust and "--exact" in rust
    assert "1 passed; 0 failed; 0 ignored;" in rust and "NOT passed" in rust
    expected = {
        "config::tests::login_diagnostics_is_explicit_and_backward_compatible": "config.rs",
        "config::tests::container_origins_and_executable_are_explicit": "config.rs",
        "config::tests::insecure_lan_http_is_explicit_and_backward_compatible": "config.rs",
        "config::tests::insecure_lan_http_requires_all_switches_and_exact_ipv4_range": "config.rs",
        "config::tests::insecure_lan_opt_in_preserves_https_and_loopback_policy": "config.rs",
        "diagnostics::tests::exact_schema_roundtrip_and_strict_bounds": "diagnostics.rs",
        "diagnostics::tests::optional_action_fields_preserve_v1_and_reject_unsafe_values": "diagnostics.rs",
        "error::tests::source_metadata_is_finite_and_absent_by_default": "error.rs",
        "app::runtime_tests::secure_cookie_has_same_creation_deletion_policy": "app.rs",
        "helper::container_command_is_explicit_and_native_command_keeps_namespace": "helper.rs",
        "tests::cookie_structure_requires_finite_expiry_but_not_live_session": "tests.rs",
        "tests::cookie_selection_obeys_domain_secure_expiry_path_and_precedence": "tests.rs",
        "tests::insecure_lan_http_login_session_logout_preserves_security_contract": "tests.rs",
        "upstream_body::tests::gzip_is_bounded_and_requires_complete_valid_stream": "upstream_body.rs",
        "log_settings::tests::strict_defaults_thresholds_and_fixed_errors": "log_settings.rs",
        "log_settings::tests::update_commit_hot_watch_failure_keeps_old_and_reopen": "log_settings.rs",
        "helper::tests::session_payload_prunes_expired_and_canonicalizes_session_expiry": "helper.rs",
        "helper::tests::session_payload_expired_is_not_malformed_but_unsafe_fields_still_are": "helper.rs",
        "config::tests::gateway_settings_reject_aliases_and_configuration_destination": "config.rs",
        "gateway_settings::tests::private_defaults_reopen_and_failed_update_keep_snapshot": "gateway_settings.rs",
        "gateway_settings::tests::corrupt_versions_unknown_fields_and_symlinks_fail_closed": "gateway_settings.rs",
        "responses_compat::tests::preserves_raw_values_and_only_inserts_missing_root_key": "responses_compat.rs",
        "responses_compat::tests::rejects_ambiguous_encoding_duplicates_integrity_and_bounds": "responses_compat.rs",
        "responses_compat::tests::auto_classifies_only_single_surviving_leading_product": "responses_compat.rs",
        "request_log::tests::forwarding_mode_is_optional_for_old_logs_and_strict_when_present": "request_log.rs",
        "tests::gateway_tests::buffered_json_deadlines_are_total_and_cancel_safe": "gateway_tests.rs",
    }
    selected = re.findall(r"^    ([a-zA-Z_][\w:]*)[ \t]*\\?$", rust, re.M)
    assert len(selected) == len(expected) == 26 and set(selected) == set(expected)
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
        assert not visibility.public_existing()
        response.read.return_value = b'{"visibility":"public"}'
        assert visibility.public_existing()
        request = fetch.call_args.args[0]
        assert request.full_url == "https://api.github.com/users/fixtureowner/packages/container/fixturerepo"
        assert request.get_method() == "GET" and request.data is None
        assert request.get_header("Authorization") == "Bearer SENTINEL-CI-TOKEN"
        assert fetch.call_args.kwargs == {"timeout": 15}
        response.read.assert_called_with(65537)
        with patch.dict(visibility.os.environ, {"OWNER_TYPE": "Organization"}):
            assert visibility.public_existing()
            assert fetch.call_args.args[0].full_url == "https://api.github.com/orgs/fixtureowner/packages/container/fixturerepo"
        response.read.return_value = b'{"visibility":"internal"}'
        assert not visibility.public_existing()
        response.read.return_value = b'{}'
        assert not visibility.public_existing()
        for status in (401, 403, 404, 429, 500):
            fetch.side_effect = visibility.HTTPError("https://fixture.invalid", status, "SENTINEL", {}, None)
            assert not visibility.public_existing()
    for invalid in ({"OWNER_TYPE": "Unknown"}, {"REPOSITORY": "missing-owner"}, {"REPOSITORY": "/package"}):
        with patch.dict(visibility.os.environ, {**env, **invalid}), patch.object(visibility, "urlopen") as fetch:
            assert not visibility.public_existing()
            fetch.assert_not_called()
    # Execute the real CLI boundary with mocked HTTP; exceptions/API bodies and
    # token sentinels must never reach stdout/stderr, even for invalid responses.
    for scenario in ("public", "private", "internal", "missing", "malformed", "wrong_shape", "not_found", "denied", "network", "unexpected_argument"):
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.dict(os.environ, env), patch("urllib.request.urlopen") as fetch, \
                patch("sys.argv", ["ghcr_visibility.py"] + (["--allow-new"] if scenario == "unexpected_argument" else [])), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            response = fetch.return_value.__enter__.return_value
            response.read.return_value = {
                "malformed": b'SENTINEL-CI-TOKEN', "wrong_shape": b'[]', "missing": b'{}',
            }.get(scenario, json.dumps({"visibility": scenario, "private_body": "SENTINEL-CI-TOKEN"}).encode())
            if scenario in ("not_found", "denied"):
                fetch.side_effect = visibility.HTTPError("https://fixture.invalid", 404 if scenario == "not_found" else 403,
                                                       "SENTINEL-CI-TOKEN", {}, None)
            elif scenario == "network":
                fetch.side_effect = RuntimeError("SENTINEL-CI-TOKEN")
            try:
                runpy.run_path(str(ROOT / "docker/ghcr_visibility.py"), run_name="__main__")
            except SystemExit as exit_status:
                assert exit_status.code == (0 if scenario == "public" else 1)
            else:
                raise AssertionError("visibility CLI did not exit")
            if scenario == "unexpected_argument":
                fetch.assert_not_called()
        assert stdout.getvalue() == json.dumps({"ghcr_visibility": "passed" if scenario == "public" else "blocked"}) + "\n"
        assert stderr.getvalue() == "" and "SENTINEL" not in stdout.getvalue()
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
                if scenario == 'browser_failed':
                    body = b'{"supervisor_probe":"failed","stage":"browser_normal","browser_failure":{"fixture":"failed","phase":"proactive_self","category":"assertion","assertion_kind":"helper_result","helper_error":"user_self_unverified"}}'
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
            if scenario == 'browser_failed':
                assert report['failure']['probe_failure']['browser_failure']['helper_error'] == 'user_self_unverified'
        lifecycle_calls = [args for args in calls if args[1] != "inspect"]
        assert lifecycle_calls[-1][1] == "rm"
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

    # Three probe sources share one stdin transport. Credentials travel only in
    # the later HTTP invocation, not argv, environment, or installed source.
    calls = []

    def transport_command(args, **kwargs):
        calls.append((args, kwargs))
        return SimpleNamespace(returncode=0, stdout=b"", stderr=b"")

    with patch.object(smoke_module, "command", transport_command):
        for probe in smoke_module.PROBES:
            smoke_module.install_probe("fixture-container", probe, smoke_module.Evidence())
    assert len(calls) == 3
    for (args, kwargs), probe in zip(calls, smoke_module.PROBES):
        assert args == ["docker", "exec", "--interactive", "fixture-container", "python", "-B",
                        "-c", smoke_module.PROBE_WRITER, probe]
        assert kwargs["input"] == (ROOT / "tests/container" / probe).read_bytes()
        assert kwargs["timeout"] == 15 and "env" not in kwargs
    assert '"docker", "cp"' not in smoke
    checks += 1

    # Execute the actual fixed writer against in-memory stdin/file mocks, never
    # local /tmp probe files. Creation is exclusive, no-follow, and private.
    from unittest.mock import MagicMock
    source = b'print("synthetic probe")\n'
    for probe in smoke_module.PROBES:
        stream = MagicMock()
        with patch.object(smoke_module.sys, "argv", ["-c", probe]), \
                patch.object(smoke_module.sys, "stdin", SimpleNamespace(buffer=io.BytesIO(source))), \
                patch.object(smoke_module.os, "open", return_value=71) as opened, \
                patch.object(smoke_module.os, "fdopen", return_value=stream):
            exec(compile(smoke_module.PROBE_WRITER, "<fixed-probe-writer>", "exec"), {})
        opened.assert_called_once_with("/tmp/" + probe, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        stream.__enter__.return_value.write.assert_called_once_with(source)
    for probe, content in (("../arbitrary", source), ("http_probe.py", b""), ("http_probe.py", b"x" * 65537)):
        with patch.object(smoke_module.sys, "argv", ["-c", probe]), \
                patch.object(smoke_module.sys, "stdin", SimpleNamespace(buffer=io.BytesIO(content))), \
                patch.object(smoke_module.os, "open") as opened:
            try:
                exec(compile(smoke_module.PROBE_WRITER, "<fixed-probe-writer>", "exec"), {})
            except AssertionError:
                pass
            else:
                raise AssertionError("unsafe probe transport accepted")
            opened.assert_not_called()
    checks += 1

    # A transport denial, stopped backend, or invalid zero-exit probe output
    # must block publication and preserve fixed evidence BEFORE Docker removal.
    for scenario in ("readonly_transport", "stopped", "invalid_json", "transport_timeout", "supervisor_transport", "browser_transport"):
        calls = []
        sentinel = "SENTINEL-private-never-reflect"

        def diagnostic_command(args, **kwargs):
            calls.append(args)
            code, body, stderr = 0, b"", b""
            assert sentinel not in " ".join(args)
            if args[1:3] == ["image", "inspect"]:
                body = ("sha256:" + "a" * 64 + "\n").encode()
            elif args[1] == "inspect":
                assert args[2:4] == ["--format", smoke_module.STATE_FORMAT]
                body = b"exited 1\n" if scenario == "stopped" else b"running 0\n"
            elif args[1] == "exec" and "-c" in args:
                assert sentinel.encode() not in kwargs["input"] and "env" not in kwargs
                if scenario == "transport_timeout":
                    raise smoke_module.subprocess.TimeoutExpired(args, 15, output=sentinel, stderr=sentinel)
                if scenario in ("readonly_transport", "stopped") or (
                        scenario == "supervisor_transport" and args[-1] == "supervisor_probe.py") or (
                        scenario == "browser_transport" and args[-1] == "browser_probe.py"):
                    code = 1
                    stderr = (("container is not running " if scenario == "stopped" else "Read-only file system ") + sentinel).encode()
            elif args[-1] == "/tmp/http_probe.py":
                assert json.loads(kwargs["input"]) == {"root": sentinel}
                body = sentinel.encode() if scenario == "invalid_json" else b'{"http_probe":"passed"}'
            return SimpleNamespace(returncode=code, stdout=body, stderr=stderr)

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "outputs"
            with patch.object(smoke_module, "command", diagnostic_command), \
                    patch.object(smoke_module.os, "getuid", return_value=1000), \
                    patch.object(smoke_module.secrets, "token_urlsafe", return_value=sentinel), \
                    patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}), \
                    redirect_stdout(io.StringIO()) as printed:
                status = smoke_module.main("fixture:ci")
            report = json.loads(printed.getvalue())
            assert status == 1 and report["publish_ready"] is False and report["cleanup"] is True
            assert output.read_text() == "publish_ready=false\n"
            assert sentinel not in printed.getvalue()
            failure = report["failure"]
            assert set(failure) == {"cmdkind", "returncode", "category", "container_state", "container_exitcode"}
            assert failure["category"] == {
                "readonly_transport": "read_only_filesystem", "stopped": "container_not_running",
                "invalid_json": "invalid_probe_response", "transport_timeout": "command_timeout",
                "supervisor_transport": "read_only_filesystem", "browser_transport": "read_only_filesystem"}[scenario]
            assert failure["cmdkind"] == ("http_probe" if scenario == "invalid_json" else "probe_install")
            expected_stage = "http_auth" if scenario == "invalid_json" else (
                "supervisor_browser_cleanup" if scenario in ("supervisor_transport", "browser_transport") else "http_probe_install")
            assert report["stage"] == expected_stage
            assert failure["container_state"] == ("exited" if scenario == "stopped" else "running")
            assert failure["container_exitcode"] == (1 if scenario == "stopped" else 0)
            assert failure["returncode"] == (None if scenario == "transport_timeout" else 0 if scenario == "invalid_json" else 1)
            assert calls[-2][1] == "inspect" and calls[-1][1] == "rm"
            assert not any(args[-1] == "--capability" for args in calls)
    checks += 1

    # Malicious/oversized inspect or stderr output cannot become report text.
    evidence = smoke_module.Evidence()
    for body in (b"SENTINEL-private-state", b"running 0\n" + b"x" * 200, b"running 999\n"):
        with patch.object(smoke_module, "command", return_value=SimpleNamespace(returncode=0, stdout=body)):
            detail = evidence.failure("fixture", True)
        assert detail["container_exitcode"] is None and "SENTINEL" not in json.dumps(detail)
    assert smoke_module.error_category(b"SENTINEL-private-docker-error") == "command_failed"
    assert smoke_module.error_category(b"x" * 8192 + b"permission denied") == "command_failed"
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
    assert 'from local_browser_smoke import notice_backdrop_cases' in browser_probe
    assert browser_probe.index('while not Path("finish").exists():') < browser_probe.index(
        'await notice_backdrop_cases(page, report, render)') < browser_probe.index('await context.close()')
    assert browser_probe.index('await notice_backdrop_cases(page, report, render)') < browser_probe.index(
        'await navigation_readiness_case(page, report)') < browser_probe.index('await context.close()')
    assert browser_probe.index('await navigation_readiness_case(page, report)') < browser_probe.index(
        'await proactive_self_case(browser, context, page, report)') < browser_probe.index('await context.close()')
    assert 'assert report == {"native_notice_backdrop_cases": 10, "native_navigation_readiness_cases": 1, "native_proactive_self_cases": 1}' in browser_probe
    diagnostic_envelopes(smoke_module, native, browser_notice_wiring())
    checks += 1
    assert '/tmp/browser_probe.py' in probe and 'start_worker(helper, directory)' in probe
    assert '"/tmp/browser_probe.py"' not in smoke  # never docker-exec Python browser directly
    checks += 1
    print(json.dumps({"container_contract": "passed", "checks": checks, "mock_static_only": True}))


if __name__ == "__main__":
    main()
