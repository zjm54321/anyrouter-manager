"""Explicit local-only real CloakBrowser probe; never production login.

Run ONLY inside the documented private PID namespace. No unittest discovery.
"""
import asyncio
import importlib.metadata
import inspect
import json
import os
from pathlib import Path
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from browser_helper import core
from browser_helper.__main__ import reserve_protocol_fd, write_all

BINARY = '/nix/store/sv231g34qjil8r23fc43i1mnlmyvgzhn-cloakbrowser-chromium-146.0.7680.177.5/bin/cloakbrowser-chrome'


def audit(event, args):
    if event == 'socket.connect':
        address = args[1]
        if isinstance(address, tuple) and address[0] not in ('127.0.0.1', '::1'):
            raise RuntimeError('nonlocal_network_forbidden')


async def probe(report):
    import cloakbrowser
    from playwright.async_api import Browser
    import greenlet
    assert importlib.metadata.version('cloakbrowser') == '0.5.11'
    assert importlib.metadata.version('playwright') == '1.58.0'
    assert inspect.iscoroutinefunction(cloakbrowser.launch_async)
    assert greenlet.greenlet(lambda: True).switch()
    report['native_import'] = True
    second_requests = []
    requests = []
    mode = {'kind': 'early', 'slow_finished': False}

    class Second(BaseHTTPRequestHandler):
        def do_GET(self):
            second_requests.append('GET')
            self.send_response(200)
            self.end_headers()
        do_POST = do_GET
        def log_message(self, *args):
            pass

    second = ThreadingHTTPServer(('127.0.0.1', 0), Second)

    class First(BaseHTTPRequestHandler):
        def reply(self, status, body=b'', headers=()):
            self.send_response(status)
            for key, value in headers:
                self.send_header(key, value)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            requests.append(('GET', self.path))
            if self.path == '/login':
                js = ("await fetch('/unrelated', {method:'POST',body:'fictional-sentinel'});"
                      "const reply=await fetch('/api/user/login',{method:'POST',body:'fictional-sentinel'});"
                      "const result=await reply.json();if(!result.success)return;"
                      "await new Promise(resolve=>setTimeout(resolve,400));"
                      "localStorage.setItem('user',JSON.stringify({id:42,token:'private-storage-sentinel'}));"
                      "await fetch('/ready',{method:'POST',body:'ready'});"
                      + ("history.replaceState({},'', '/console');await fetch('/optional.png').catch(()=>{});await fetch('/api/user/self');" if mode['kind'] == 'spa' else
                         ("await fetch('/api/user/self');" if mode['kind'] == 'early' else '')))
                html = ("<form class='semi-form'><input id='username'><input id='password' type='password'>"
                        "<button type='submit'>login</button></form><script>document.querySelector('form').onsubmit=async e=>{"
                        "e.preventDefault();" + js + "};</script>").encode()
                self.reply(200, html, [('Content-Type', 'text/html')])
            elif self.path == '/console':
                if not mode.get('spa_ready', False) or mode['kind'] == 'slow' and not mode['slow_finished']:
                    mode['premature_console'] = True
                self.reply(200, b"<img src='/optional.png'><script>fetch('/api/user/self')</script>", [('Content-Type', 'text/html')])
            elif self.path == '/api/user/self':
                self.reply(200, b'{"success":true,"data":{"id":42}}', [('Content-Type', 'application/json'), ('Set-Cookie', 'api_fixture=local; Path=/api; HttpOnly')])
            elif self.path == '/optional.png':
                self.reply(302, headers=[('Location', f'http://127.0.0.1:{second.server_port}/sink')])
            else:
                self.reply(200, b"<title>local-cookie-probe</title><script>document.cookie='js_fixture=local; Path=/'</script>",
                           [('Content-Type', 'text/html'), ('Set-Cookie', 'api_fixture=local; Path=/api; HttpOnly')])

        def do_POST(self):
            self.rfile.read(int(self.headers.get('Content-Length', 0)))
            requests.append(('POST', self.path))
            if self.path in ('/307', '/308'):
                self.reply(int(self.path[1:]), headers=[('Location', f'http://127.0.0.1:{second.server_port}/sink')])
            elif self.path == '/api/user/login':
                if mode['kind'] == 'slow':
                    time.sleep(6)
                mode['slow_finished'] = True
                body = (b'{"success":false,"message":"private-body-sentinel","token":"private-token-sentinel","private":"private-field-sentinel"}'
                        if mode['kind'] == 'rejected' else b'{"success":true}')
                self.reply(200, body, [('Content-Type', 'application/json')])
            elif self.path == '/unrelated':
                self.reply(200, b'{}', [('Content-Type', 'application/json')])
            elif self.path == '/ready':
                mode['spa_ready'] = True
                self.reply(200, b'{}', [('Content-Type', 'application/json')])
            else:
                self.reply(404)

        def log_message(self, *args):
            pass

    first = ThreadingHTTPServer(('127.0.0.1', 0), First)
    servers = [first, second]
    threads = [threading.Thread(target=s.serve_forever, daemon=True) for s in servers]
    for thread in threads:
        thread.start()
    origin = f'http://127.0.0.1:{first.server_port}'

    def local_safe(url, path=None):
        parsed = urlsplit(url)
        return (parsed.scheme == 'http' and parsed.hostname == '127.0.0.1'
                and parsed.port == first.server_port and parsed.username is None
                and parsed.password is None and not parsed.fragment
                and (path is None or parsed.path == path))

    async def launch(**kwargs):
        # No added --no-sandbox. Wrapper 0.5.11 adds it by default (config.py).
        return await cloakbrowser.launch_async(**kwargs, timeout=30000, args=[
            '--disable-background-networking', '--disable-component-update',
            '--disable-domain-reliability', '--no-first-run',
            '--proxy-server=http://127.0.0.1:9', '--proxy-bypass-list=127.0.0.1',
            '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1'])

    browser = context = None
    try:
        browser = await launch(headless=True)
        assert isinstance(browser, Browser)
        context = await browser.new_context(service_workers='block')
        page = await context.new_page()
        await page.goto('about:blank')
        assert await page.evaluate("document.title='local-blank-probe'; 6*7") == 42
        assert await page.title() == 'local-blank-probe'
        report['wrapper_browser_blank_js_title'] = True

        redirected = []
        async def guard(route):
            if not local_safe(route.request.url):
                await route.abort()
                return
            fetched = await route.fetch(max_redirects=0, max_retries=0, timeout=10000)
            if 300 <= fetched.status < 400:
                redirected.append(fetched.status)
                await route.abort()
            else:
                await route.fulfill(response=fetched)
        await context.route('**/*', guard)
        await page.goto(origin + '/cookies')
        assert await page.title() == 'local-cookie-probe'
        cookies = await context.cookies()
        assert any(c['name'] == 'js_fixture' and c['path'] == '/' for c in cookies)
        assert any(c['name'] == 'api_fixture' and c['path'] == '/api' for c in cookies)
        report['real_all_context_cookie_paths'] = True
        for status in (307, 308):
            rejected = await page.evaluate("""async path => {
                try { await fetch(path,{method:'POST',body:'fictional-sentinel'}); return false; }
                catch { return true; }
            }""", '/' + str(status))
            assert rejected
        assert redirected == [307, 308]
        assert second_requests == []
        report['real_307_308_second_origin_requests'] = 0
        await context.close()
        context = None
        await browser.close()
        browser = None
        report['wrapper_close'] = True

        # Production helper logic unchanged. ONLY this fixture temporarily swaps
        # fixed origin validation/cookie fixture domain; never production URLs.
        original_filter = core.filtered_cookies
        original_safe = core.safe_url
        def local_cookies(cookies, source):
            assert local_safe(source, '/console')
            translated = [dict(c, domain='anyrouter.top') for c in cookies if c['domain'] == '127.0.0.1']
            with patch.object(core, 'safe_url', original_safe):
                return original_filter(translated, 'https://anyrouter.top/console')
        with patch.object(core, 'ORIGIN', origin), patch.object(core, 'safe_url', local_safe), patch.object(core, 'filtered_cookies', local_cookies), patch.object(core, 'browser_launcher', return_value=launch):
            for kind in ('early', 'slow', 'spa'):
                report['phase'] = 'helper_' + kind
                mode.update(kind=kind, slow_finished=False, premature_console=False, spa_ready=False)
                console_before = requests.count(('GET', '/console'))
                start = time.monotonic()
                result = await core.run_login(core.LoginInput('fixture-user', 'fictional-sentinel', 20000))
                assert result['ok'], result.get('error', 'fixture_failed')
                assert result['api_user'] == '42'
                assert any(c['path'] == '/api' for c in result['cookies'])
                assert not mode['premature_console']
                if kind == 'slow':
                    assert mode['slow_finished'] and time.monotonic() - start >= 6
                if kind == 'spa':
                    assert requests.count(('GET', '/console')) == console_before
                report['real_helper_' + kind + '_self'] = True
            mode.update(kind='rejected', slow_finished=False)
            with patch.dict(os.environ, {'BROWSER_HELPER_DIAGNOSTICS': '1'}):
                result = await core.run_login(core.LoginInput('private-user-sentinel', 'private-password-sentinel', 20000))
            assert result['error'] == 'login_failed' and result['diagnostics']['login_success'] is False
            assert result['diagnostics']['login_status'] == 200 and result['diagnostics']['login_json'] is True
            assert result['diagnostics']['self_requested'] is False and 'cookies' not in result
            encoded = json.dumps(result)
            assert all(secret not in encoded for secret in ('private-user-sentinel', 'private-password-sentinel', 'private-body-sentinel', 'private-token-sentinel', 'private-field-sentinel'))
            report['real_helper_rejected_json_safe_diagnostics'] = True
        report.pop('phase', None)
        assert second_requests == []
    finally:
        cleanup_failed = False
        for resource, timeout in ((context, 5), (browser, 10)):
            if resource is not None:
                try:
                    await asyncio.wait_for(resource.close(), timeout)
                except Exception:
                    cleanup_failed = True
        for server in servers:
            await asyncio.to_thread(server.shutdown)
            server.server_close()
        for thread in threads:
            thread.join(timeout=2)
        if cleanup_failed:
            raise RuntimeError('local_cleanup_failed')


def main():
    fd = reserve_protocol_fd()
    report = {'ok': False, 'local_only': True, 'wrapper_default_no_sandbox': True}
    try:
        host_ns = os.environ.get('HELPER_SMOKE_HOST_PIDNS')
        if not host_ns or os.readlink('/proc/self/ns/pid') == host_ns:
            raise RuntimeError('private_pid_namespace_required')
        if not Path(BINARY).is_file() or not os.access(BINARY, os.X_OK):
            raise RuntimeError('patched_binary_missing')
        os.environ['CLOAKBROWSER_BINARY_PATH'] = BINARY
        os.environ['CLOAKBROWSER_AUTO_UPDATE'] = 'false'
        sys.addaudithook(audit)
        asyncio.run(asyncio.wait_for(probe(report), 150))
        report['ok'] = True
    except Exception as exc:
        report['error_type'] = type(exc).__name__
        # Only controlled infrastructure messages; never browser log/HTML/body.
        text = str(exc)
        if 'libstdc++.so.6' in text:
            report['error'] = 'ImportError: libstdc++.so.6: cannot open shared object file: No such file or directory'
        elif text in ('private_pid_namespace_required', 'patched_binary_missing'):
            report['error'] = text
        else:
            report['error'] = 'local_browser_probe_failed'
    write_all(fd, (json.dumps(report, separators=(',', ':')) + '\n').encode())
    os.close(fd)
    return 0 if report['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
