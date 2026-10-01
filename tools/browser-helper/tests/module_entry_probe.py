"""Local-only diagnostic of the EXACT python -m browser_helper process entry.

Parent creates a temporary sitecustomize fixture; production config unchanged.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
BINARY = '/nix/store/sv231g34qjil8r23fc43i1mnlmyvgzhn-cloakbrowser-chromium-146.0.7680.177.5/bin/cloakbrowser-chrome'


def configure_local_fixture():
    import atexit
    import asyncio
    import inspect
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from urllib.parse import urlsplit
    from browser_helper import core

    diagnostic_path = os.environ['HELPER_ENTRY_DIAGNOSTIC']
    details = {'fixture_loaded': True, 'binary_exists': Path(os.environ.get('CLOAKBROWSER_BINARY_PATH', '')).is_file(),
               'binary_executable': os.access(os.environ.get('CLOAKBROWSER_BINARY_PATH', ''), os.X_OK),
               'node_exists': Path(os.environ.get('PLAYWRIGHT_NODEJS_PATH', '')).is_file(),
               'ld_library_path_present': bool(os.environ.get('LD_LIBRARY_PATH')),
               'private_pid_namespace': os.readlink('/proc/self/ns/pid') != os.environ['HELPER_SMOKE_HOST_PIDNS'],
               'proxy_env_present': any(os.environ.get(k) for k in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'))}

    def save():
        Path(diagnostic_path).write_text(json.dumps(details))
    save()

    class Local(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == '/login':
                body = b"<form class='semi-form'><input id='username'><input id='password' type='password'><button type='submit'>fixture</button></form><script>document.querySelector('form').onsubmit=async e=>{e.preventDefault();const reply=await fetch('/api/user/login',{method:'POST',body:'fictional-sentinel'});if(!(await reply.json()).success)return;await new Promise(resolve=>setTimeout(resolve,400));localStorage.setItem('user',JSON.stringify({id:42,token:'private-storage-sentinel'}));await fetch('/api/user/self');};</script>"
                content_type = 'text/html'
            elif self.path == '/console':
                body = b"<script>fetch('/api/user/self')</script>"
                content_type = 'text/html'
            elif self.path == '/api/user/self':
                body = b'{"success":true,"data":{"id":42}}'
                content_type = 'application/json'
                details['local_profile_requested'] = True
            else:
                body, content_type = b'', 'text/plain'
            self.send_response(200)
            self.send_header('Content-Type', content_type)
            self.send_header('Content-Length', str(len(body)))
            if self.path == '/api/user/self':
                self.send_header('Set-Cookie', 'fixture_session=local; Path=/api; HttpOnly')
            self.end_headers()
            self.wfile.write(body)
        def do_POST(self):
            self.rfile.read(int(self.headers.get('Content-Length', 0)))
            assert self.path == '/api/user/login'
            details['local_login_requested'] = True
            body = b'{"success":false,"message":"private-body-sentinel","token":"private-token-sentinel"}' if os.environ.get('HELPER_FIXTURE_REJECT') == '1' else b'{"success":true}'
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Local)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    def cleanup():
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
        save()
    atexit.register(cleanup)
    origin = f'http://127.0.0.1:{server.server_port}'
    def local_safe(url, path=None):
        parsed = urlsplit(url)
        return (parsed.scheme == 'http' and parsed.hostname == '127.0.0.1' and parsed.port == server.server_port
                and parsed.username is None and parsed.password is None and not parsed.fragment
                and (path is None or parsed.path == path))

    original_filter, original_safe = core.filtered_cookies, core.safe_url
    def local_filter(cookies, source):
        if not local_safe(source, '/console'):
            return []
        translated = [dict(c, domain='anyrouter.top') for c in cookies if c['domain'] == '127.0.0.1']
        core.safe_url = original_safe
        try:
            return original_filter(translated, 'https://anyrouter.top/console')
        finally:
            core.safe_url = local_safe

    original_launcher = core.browser_launcher
    def diagnosed_launcher():
        try:
            launch = original_launcher()
            details['wrapper_import_and_precheck'] = True
            details['launch_async_coroutine'] = inspect.iscoroutinefunction(launch)
        except Exception as exc:
            details.update(launch_error_type=type(exc).__name__, launch_error_code='precheck_or_import_failed')
            save()
            raise
        async def diagnosed_launch(**kwargs):
            details['fd1_devnull'] = os.readlink('/proc/self/fd/1') == '/dev/null'
            details['fd2_devnull'] = os.readlink('/proc/self/fd/2') == '/dev/null'
            details['extra_noninheritable_fds'] = sum(
                1 for entry in Path('/proc/self/fd').iterdir()
                if int(entry.name) > 2 and entry.exists() and not os.get_inheritable(int(entry.name)))
            save()
            try:
                browser = await launch(**kwargs, timeout=30000, args=[
                    '--disable-background-networking', '--disable-component-update', '--disable-domain-reliability', '--no-first-run',
                    '--proxy-server=http://127.0.0.1:9', '--proxy-bypass-list=127.0.0.1',
                    '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1'])
                details['actual_browser_launched'] = True
                original_close = browser.close
                async def close(*args, **kwargs):
                    await original_close(*args, **kwargs)
                    details['actual_browser_closed'] = True
                    save()
                browser.close = close
                save()
                return browser
            except Exception as exc:
                text = str(exc)
                code = 'local_launch_failed'
                if 'libstdc++.so.6' in text:
                    code = 'missing_libstdc++'
                elif 'ENOENT' in text:
                    code = 'driver_executable_missing'
                elif 'Operation not permitted' in text:
                    code = 'namespace_permission_denied'
                details.update(launch_error_type=type(exc).__name__, launch_error_code=code)
                save()
                raise
        return diagnosed_launch

    def audit(event, args):
        if event == 'socket.connect' and isinstance(args[1], tuple) and args[1][0] not in ('127.0.0.1', '::1'):
            raise RuntimeError('nonlocal_network_forbidden')
    sys.addaudithook(audit)
    core.ORIGIN = origin
    core.safe_url = local_safe
    core.filtered_cookies = local_filter
    core.browser_launcher = diagnosed_launcher


def main():
    report = {'ok': False, 'local_only': True}
    with tempfile.TemporaryDirectory(prefix='helper-local-entry-', dir='/tmp/opencode') as directory:
        temporary = Path(directory)
        env = dict(os.environ, UV_CACHE_DIR=str(temporary / 'uv-cache'), UV_NO_SYNC='1',
                   CLOAKBROWSER_BINARY_PATH=os.environ.get('CLOAKBROWSER_BINARY_PATH') or BINARY,
                   CLOAKBROWSER_AUTO_UPDATE='false', PYTHONDONTWRITEBYTECODE='1',
                   HELPER_SMOKE_HOST_PIDNS=os.readlink('/proc/self/ns/pid'))
        command = ['unshare', '--user', '--map-current-user', '--pid', '--fork', '--kill-child=KILL', '--mount-proc', '--',
                   'uv', 'run', '--offline', '--frozen', '--no-sync', 'python', '-B', '-m', 'browser_helper']
        invalid = subprocess.run(command, input=b'{}\n', capture_output=True, cwd=ROOT, env=env, timeout=20)
        report['invalid_input'] = (invalid.returncode == 1 and invalid.stderr == b'' and
                                   json.loads(invalid.stdout) == {'ok': False, 'error': 'invalid_input'})
        (temporary / 'sitecustomize.py').write_text('from module_entry_probe import configure_local_fixture\nconfigure_local_fixture()\n')
        env.update(PYTHONPATH=os.pathsep.join([str(temporary), str(ROOT / 'tests'), str(ROOT)]),
                   HELPER_ENTRY_DIAGNOSTIC=str(temporary / 'diagnostic.json'))
        actual = subprocess.run(command, input=b'{"username":"fixture-user","password":"fictional-sentinel","timeout_ms":45000}\n',
                                capture_output=True, cwd=ROOT, env=env, timeout=65)
        result = json.loads(actual.stdout)
        report.update(entry_returncode=actual.returncode, stdout_single_json_line=actual.stdout.count(b'\n') == 1,
                      stderr_empty=actual.stderr == b'', helper_ok=result.get('ok') is True)
        if not result.get('ok'):
            report['helper_error'] = result.get('error')
        else:
            report['profile_id_confirmed'] = result.get('api_user') == '42'
            report['api_cookie_path_preserved'] = any(c['path'] == '/api' for c in result['cookies'])
        diagnostic = temporary / 'diagnostic.json'
        if diagnostic.exists():
            report['diagnostic'] = json.loads(diagnostic.read_text())
        report['ok'] = (report['invalid_input'] and report['helper_ok'] and report['stdout_single_json_line']
                         and report['stderr_empty'] and report['profile_id_confirmed'] and report['api_cookie_path_preserved'])
        env.update(HELPER_FIXTURE_REJECT='1', BROWSER_HELPER_DIAGNOSTICS='1')
        rejected = subprocess.run(command, input=b'{"username":"private-user-sentinel","password":"private-password-sentinel","timeout_ms":45000}\n', capture_output=True, cwd=ROOT, env=env, timeout=65)
        failure = json.loads(rejected.stdout)
        from browser_helper import core
        report['rejected_single_json_safe_diagnostics'] = (
            rejected.returncode == 1 and rejected.stderr == b'' and rejected.stdout.count(b'\n') == 1
            and failure['error'] == 'login_failed' and 'cookies' not in failure
            and failure['diagnostics'] == core.diagnostics(dict(phase='profile_wait', page='login', login_requested=True, login_status=200, login_json=True, login_success=False))
            and all(value.encode() not in rejected.stdout for value in ('private-user-sentinel', 'private-password-sentinel', 'private-body-sentinel', 'private-token-sentinel')))
        report['ok'] = report['ok'] and report['rejected_single_json_safe_diagnostics']
    print(json.dumps(report, separators=(',', ':')))
    return 0 if report['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
