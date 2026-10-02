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


async def notice_backdrop_cases(page, report, render):
    # Only synthetic DOM: no credentials, submit events, or upstream requests.
    cases = [(title, '', delayed, True) for title in ('系统公告', 'System Notice')
             for delayed in (False, True)]
    cases += [('System Notification', '', False, False)]
    cases += [('System Notice', body, False, False) for body in
              ('Please accept the agreement and terms', 'Verify captcha',
               '<input>', '<iframe></iframe>', '<div class="g-recaptcha"></div>')]
    for index, (title, body, delayed, expected) in enumerate(cases):
        report['phase'] = 'notice_backdrop_' + str(index)
        content = (
            '<div class="semi-modal-content" style="position:relative;z-index:1">'
            f'<h5 class="semi-modal-title" style="display:flex">{title}</h5>{body}'
            '<button type="button" class="semi-modal-close" '
            'onclick="window.closedCount++;document.querySelector(\'#notice-shell\').remove()">'
            '<span class="semi-icon-close">X</span></button></div>')
        await render(
            '<script>window.closedCount=0;window.submitCount=0</script>'
            '<form onsubmit="event.preventDefault();window.submitCount++">'
            '<input id="username"><input id="password" type="password">'
            '<button id="continue" type="submit">Continue</button></form>'
            '<div id="notice-shell" style="position:fixed;inset:0;z-index:100">'
            '<div id="backdrop" style="position:absolute;inset:0;background:white"></div>'
            + (f'<template id="pending-modal">{content}</template>' if delayed else
               f'<div role="dialog">{content}</div>') + '</div>')
        button = await core.owned_submit(page, page.locator('#username'), page.locator('#password'))
        assert button is not None
        unobstructed = """b => {
            const r=b.getBoundingClientRect();
            const hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);
            return hit===b || b.contains(hit);
        }"""
        assert not await button.evaluate(unobstructed)
        if delayed:
            # Deterministic fixture mounting, NOT a production wait/race fix.
            assert await page.locator('[role="dialog"]').count() == 0
            await page.evaluate("""() => {
                const dialog=document.createElement('div');
                dialog.setAttribute('role','dialog');
                dialog.append(document.querySelector('#pending-modal').content.cloneNode(true));
                document.querySelector('#notice-shell').append(dialog);
            }""")
        dialog = page.locator('[role="dialog"]')
        await dialog.wait_for(state='visible', timeout=3000)
        assert await dialog.locator('h5.semi-modal-title').inner_text() == title
        assert await dialog.evaluate(core.NOTICE_ELIGIBLE_JS) == expected
        state = dict(deadline=asyncio.get_running_loop().time()+5, submitted=False)
        assert await core.dismiss_notice_once(page, state) == expected
        assert not await core.dismiss_notice_once(page, state)
        assert await page.evaluate('window.closedCount') == int(expected)
        if expected:
            assert await page.locator('#notice-shell').count() == 0
            assert await button.evaluate(unobstructed)
            await button.click(trial=True, timeout=3000)
        else:
            assert await dialog.is_visible()
            assert not await button.evaluate(unobstructed)
        assert await page.evaluate('window.submitCount') == 0
    report['native_notice_backdrop_cases'] = len(cases)
    report.pop('phase', None)


async def navigation_readiness_case(page, report):
    """Real DOM readiness, not a wall-clock latency or upstream login test.

    Hold a deferred script until commit is observed, then hold an image through
    DOMContentLoaded. Only after goto resolves may the production form guards
    inspect the empty fields. Everything is fulfilled locally on the same page.
    """
    url = core.ORIGIN + '/login?navigation-fixture=1'
    script_url, image_url = core.ORIGIN + '/fixture-defer.js', core.ORIGIN + '/fixture-image.svg'
    script_requested, image_requested = asyncio.Event(), asyncio.Event()
    release_script, release_image = asyncio.Event(), asyncio.Event()
    html = '''<!doctype html><html><head><script>
        window.fixture={script:false,dom:false,load:false};
        document.addEventListener('DOMContentLoaded',()=>window.fixture.dom=true);
        window.addEventListener('load',()=>window.fixture.load=true);
        </script><script defer src="/fixture-defer.js"></script></head>
        <body><img src="/fixture-image.svg"><main></main></body></html>'''

    async def document(route):
        await route.fulfill(status=200, content_type='text/html', body=html)

    async def deferred(route):
        script_requested.set()
        await release_script.wait()
        await route.fulfill(status=200, content_type='application/javascript', body='''
            window.fixture.script=true;
            document.querySelector('main').innerHTML='<form><input id="username">' +
                '<input id="password" type="password"><button type="submit">Continue</button></form>';
        ''')

    async def image(route):
        image_requested.set()
        await release_image.wait()
        await route.fulfill(status=200, content_type='image/svg+xml',
                            body='<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>')

    installed, navigation = [], None
    try:
        # Short event-driven fixture cap; no extra launch or supervisor budget.
        async with asyncio.timeout(3):
            for target, handler in ((url, document), (script_url, deferred), (image_url, image)):
                await page.route(target, handler)
                installed.append((target, handler))
            navigation = asyncio.create_task(page.goto(url, wait_until='domcontentloaded', timeout=2000))
            await script_requested.wait()
            await image_requested.wait()
            await page.wait_for_url(url, wait_until='commit', timeout=2000)
            assert not navigation.done()
            assert await page.evaluate('window.fixture') == {'script': False, 'dom': False, 'load': False}
            assert await page.locator('#username').count() == 0
            release_script.set()
            await navigation
            assert await page.evaluate('window.fixture') == {'script': True, 'dom': True, 'load': False}
            state = dict(deadline=asyncio.get_running_loop().time()+1, submitted=False,
                         network_failed=False, http_block=False)
            username, password, _ = await core.find_form(page, state)
            assert await username.input_value() == await password.input_value() == ''
            release_image.set()
            await page.wait_for_load_state('load', timeout=2000)
            assert await page.evaluate('window.fixture') == {'script': True, 'dom': True, 'load': True}
    finally:
        release_script.set()
        release_image.set()
        if navigation is not None:
            if not navigation.done():
                navigation.cancel()
            await asyncio.gather(navigation, return_exceptions=True)
        for target, handler in reversed(installed):
            await page.unroute(target, handler)
    report['native_navigation_readiness_cases'] = 1


async def proactive_self_case(browser, context, page, report):
    """One real login on loopback, borrowing ONLY the worker-owned resources."""
    from types import SimpleNamespace

    assert page.context is context and context.browser is browser
    observed = {'self': 0, 'authorized': False, 'marker_absent': True}

    class Local(BaseHTTPRequestHandler):
        def reply(self, status, body, content_type, session=False):
            self.send_response(status)
            self.send_header('Content-Type', content_type)
            self.send_header('Content-Length', str(len(body)))
            if session:
                self.send_header('Set-Cookie', 'fixture_session=synthetic; Path=/api; HttpOnly; SameSite=Strict')
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path == '/login':
                self.reply(200, b"<form class='semi-form'><input id='username'><input id='password' type='password'><button type='submit'>Continue</button></form><script>document.querySelector('form').onsubmit=async e=>{e.preventDefault();const r=await fetch('/api/user/login',{method:'POST',body:'synthetic'});if((await r.json()).success){localStorage.setItem('user',JSON.stringify({id:42}));location.href='/console';}};</script>", 'text/html')
            elif self.path == '/console':
                # Deliberately NO SPA self fetch: the helper must initiate proof.
                self.reply(200, b'<title>Synthetic console</title>', 'text/html')
            elif self.path == '/api/user/self':
                observed['self'] += 1
                observed['authorized'] = ('fixture_session=synthetic' in self.headers.get('Cookie', '')
                                          and self.headers.get('New-Api-User') == '42')
                observed['marker_absent'] &= self.headers.get('X-Helper-Proof') is None
                self.reply(200 if observed['authorized'] else 401,
                           b'{"success":true,"data":{"id":42}}' if observed['authorized'] else b'{"success":false}', 'application/json')
            else:
                self.reply(404, b'', 'text/plain')

        def do_POST(self):
            self.rfile.read(int(self.headers.get('Content-Length', 0)))
            self.reply(200 if self.path == '/api/user/login' else 404,
                       b'{"success":true}', 'application/json', session=self.path == '/api/user/login')

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Local)
    thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=.05), daemon=True)
    thread.start()
    origin = f'http://127.0.0.1:{server.server_port}'
    guards = []
    original_filter, original_safe = core.filtered_cookies, core.safe_url

    def local_safe(url, path=None):
        parsed = urlsplit(url)
        return (url in tuple(origin + endpoint for endpoint in ('/login', '/console', '/api/user/login', '/api/user/self'))
                and (path is None or parsed.path == path))

    def local_cookies(cookies, source):
        assert local_safe(source, '/console')
        translated = [dict(c, domain='anyrouter.top') for c in cookies if c['domain'] == '127.0.0.1']
        with patch.object(core, 'safe_url', original_safe):
            return original_filter(translated, 'https://anyrouter.top/console')

    async def install(pattern, handler):
        guards.append((pattern, handler))
        await context.route(pattern, handler)

    async def release():
        while guards:
            pattern, handler = guards.pop()
            await context.unroute(pattern, handler)

    async def same_page():
        return page

    async def no_close():
        pass

    async def same_context(**options):
        assert options == {'service_workers': 'block'}
        return SimpleNamespace(route=install, new_page=same_page, cookies=context.cookies, close=release)

    async def launch(**options):
        assert options == {'headless': True}
        return SimpleNamespace(new_context=same_context, close=no_close)

    try:
        async with asyncio.timeout(3):
            with patch.object(core, 'ORIGIN', origin), patch.object(core, 'safe_url', local_safe), \
                    patch.object(core, 'filtered_cookies', local_cookies), patch.object(core, 'browser_launcher', return_value=launch):
                result = await core.run_login(core.LoginInput('synthetic-user', 'synthetic-password', 2000))
            assert result['ok'] and result['api_user'] == '42'
            assert any(c['name'] == 'fixture_session' and c['http_only'] and c['path'] == '/api' for c in result['cookies'])
            assert observed == {'self': 1, 'authorized': True, 'marker_absent': True}
    finally:
        try:
            await release()
            await context.clear_cookies()
        finally:
            await asyncio.to_thread(server.shutdown)
            server.server_close()
            thread.join(timeout=1)
    report['native_proactive_self_cases'] = 1


async def ui_policy_cases(page, report, render):
    # Actual browser HTML5 .form resolution, including explicit external owners.
    fields = "<input id='username'><input id='password' type='password'>"
    wrong = "<button id='header' type='submit'>Sign in</button><form id='wrong'><button type='submit'>Login</button></form>"
    forms = [
        (f"<form id='login' class='semi-form'>{fields}<button id='good' type='submit'>Continue</button></form>", True),
        (f"<form id='login'>{fields}</form><button id='good' form='login' type='submit'>Continue</button>", True),
        (f"<div class='semi-form'>{fields}<button id='good' type='button'>Continue</button></div>", True),
        (f"<form id='login'>{fields}<button id='good' form='wrong' type='submit'>Continue</button></form>", False),
        ("<form id='login'><input id='username'><input id='password' form='wrong'><button type='submit'>Continue</button></form>", False),
        (f"{fields}<button type='submit'>Continue</button>", False),
    ]
    for index, (html, expected) in enumerate(forms):
        report['phase'] = 'form_owner_' + str(index)
        await render(wrong+html)
        report['phase'] += '_select'
        page.set_default_timeout(3000)
        button = await core.owned_submit(page, page.locator('#username'), page.locator('#password'))
        report['phase'] += '_assert'
        assert (button is not None) == expected
        if expected:
            assert await button.get_attribute('id') == 'good'
    report['native_form_owner_cases'] = len(forms)
    close = "<button type='button' class='semi-modal-close' onclick='window.closedCount++;this.parentElement.remove()'><span class='semi-icon-close'></span></button>"
    cases = [
        ('公告', '', '', True), ('通知', '', '', True), ('Notice', '', '', True),
        ('Unknown', '', '', False), ('公告验证', '', '', False),
        ('公告', 'Please accept the agreement', '', False),
        ('公告', '<input>', '', False), ('公告', '<form></form>', '', False),
        ('公告', '<iframe></iframe>', '', False), ('公告', '<div class="g-recaptcha"></div>', '', False),
        ('公告', '', '<div role="dialog">Unknown</div>', False),
        ('公告', close, '', False),
        ('公告', '登录后可以查看服务通知 / Registration and login information', '', True),
    ]
    for index, (title, body, extra, expected) in enumerate(cases):
        report['phase'] = 'notice_' + str(index)
        await render(f"<script>window.closedCount=0</script><div role='dialog' class='semi-modal-content'><h2 class='semi-modal-title'>{title}</h2>{body}{close}</div>{extra}")
        state = dict(deadline=asyncio.get_running_loop().time()+2, submitted=False)
        assert await core.dismiss_notice_once(page, state) == expected
        assert not await core.dismiss_notice_once(page, state)
        assert await page.evaluate('window.closedCount') == int(expected)
    report['native_strict_notice_cases'] = len(cases)
    for challenge in (False, True):
        report['phase'] = 'nested_notice_challenge' if challenge else 'nested_notice_plain'
        await render("<script>window.closedCount=0</script><div role='dialog'><div class='semi-modal-content'>"
                     "<h2 class='semi-modal-title'>公告</h2><p>登录后的服务通知</p>"
                     + ("<iframe></iframe>" if challenge else '')
                     + close.replace('this.parentElement.remove()', 'this.closest("[role=dialog]").remove()') + '</div></div>')
        state = dict(deadline=asyncio.get_running_loop().time()+2, submitted=False)
        assert await core.dismiss_notice_once(page, state) == (not challenge)
        assert await page.evaluate('window.closedCount') == int(not challenge)
    report['native_nested_notice_cases'] = 2
    await notice_backdrop_cases(page, report, render)
    report.pop('phase', None)


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
            if self.path == '/login?ui=1':
                self.reply(200, mode['ui_html'].encode(), [('Content-Type', 'text/html; charset=utf-8')])
                return
            if self.path == '/login':
                managed = mode['kind'] in ('managed', 'external', 'semi', 'notice')
                login_path = '/api/user/login?generated=%2Fconsole&challenge=local' if managed else '/api/user/login'
                js = ("await fetch('/unrelated', {method:'POST',body:'fictional-sentinel'});"
                      f"const reply=await fetch('{login_path}',{{method:'POST',body:'fictional-sentinel'}});"
                      "const result=await reply.json();if(!result.success)return;"
                      + ("for(const q of ['', '?auto=1']){try{await fetch('/api/user/sign_in'+q,{method:'POST'});}catch{await fetch('/auto-blocked',{method:'POST'});}}" if managed else '') +
                      "await new Promise(resolve=>setTimeout(resolve,400));"
                      "localStorage.setItem('user',JSON.stringify({id:42,token:'private-storage-sentinel'}));"
                      "await fetch('/ready',{method:'POST',body:'ready'});"
                      + ("history.replaceState({},'', '/console');await fetch('/optional.png').catch(()=>{});await fetch('/api/user/self');" if mode['kind'] == 'spa' else
                         ("await fetch('/api/user/self');" if mode['kind'] == 'early' else '')))
                fields = "<input id='username'><input id='password' type='password'>"
                button = "<button id='submit' type='submit' form='login'>Continue</button>"
                form = f"<form id='login' class='semi-form'>{fields}{button}</form>"
                if mode['kind'] == 'external':
                    form = f"<form id='login'>{fields}</form>{button}"
                if mode['kind'] == 'semi':
                    form = f"<div id='login' class='semi-form'>{fields}<button id='submit' type='button'>Continue</button></div>"
                wrong = "<button type='submit' onclick=\"fetch('/wrong-submit',{method:'POST'})\">Sign in</button><form id='wrong'><button type='submit'>Sign in</button></form>" if managed else ''
                notice = ("<div role='dialog' class='semi-modal-content' style='position:fixed;inset:0;z-index:100;background:white'>"
                          "<h2 class='semi-modal-title'>公告</h2><p>登录后可查看服务通知 / Maintenance information</p>"
                          "<button type='button' class='semi-modal-close' onclick='this.parentElement.remove()'><span class='semi-icon-close'>X</span></button></div>" if mode['kind'] == 'notice' else '')
                event = "document.querySelector('#submit').onclick" if mode['kind'] == 'semi' else "document.querySelector('#login').onsubmit"
                html = (wrong + form + notice + f"<script>{event}=async e=>{{e.preventDefault();" + js + "};</script>").encode()
                self.reply(200, html, [('Content-Type', 'text/html; charset=utf-8')])
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
            elif urlsplit(self.path).path == '/api/user/login':
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
            elif self.path == '/auto-blocked':
                self.reply(200)
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
        # Safe-page restriction stays production-only; this fixture uses loopback.
        with patch.object(core, 'safe_url', local_safe):
            async def render(html):
                mode['ui_html'] = html
                await page.goto(origin+'/login?ui=1', wait_until='domcontentloaded')
            await ui_policy_cases(page, report, render)
        await context.close()
        context = None
        await browser.close()
        browser = None
        report['wrapper_close'] = True

        # Exercise production helper logic. ONLY this fixture temporarily swaps
        # fixed origin validation/cookie fixture domain; never production URLs.
        original_filter = core.filtered_cookies
        original_safe = core.safe_url
        def local_cookies(cookies, source):
            assert local_safe(source, '/console')
            translated = [dict(c, domain='anyrouter.top') for c in cookies if c['domain'] == '127.0.0.1']
            with patch.object(core, 'safe_url', original_safe):
                return original_filter(translated, 'https://anyrouter.top/console')
        with patch.object(core, 'ORIGIN', origin), patch.object(core, 'safe_url', local_safe), patch.object(core, 'filtered_cookies', local_cookies), patch.object(core, 'browser_launcher', return_value=launch):
            for kind in ('early', 'slow', 'spa', 'managed', 'external', 'semi', 'notice'):
                report['phase'] = 'helper_' + kind
                mode.update(kind=kind, slow_finished=False, premature_console=False, spa_ready=False)
                console_before = requests.count(('GET', '/console'))
                blocked_before = requests.count(('POST', '/auto-blocked'))
                login_before = sum(method == 'POST' and urlsplit(path).path == '/api/user/login' for method, path in requests)
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
                assert sum(method == 'POST' and urlsplit(path).path == '/api/user/login' for method, path in requests) == login_before+1
                if kind in ('managed', 'external', 'semi', 'notice'):
                    assert requests.count(('POST', '/auto-blocked')) == blocked_before+2
                    assert ('POST', '/api/user/login?generated=%2Fconsole&challenge=local') in requests
                    assert not any(urlsplit(path).path in ('/api/user/sign_in', '/wrong-submit') for _, path in requests)
                report['real_helper_' + kind + '_self'] = True
            report['managed_auto_signin_sink_count'] = 0
            report['managed_login_query_preserved_single_submit'] = True
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
