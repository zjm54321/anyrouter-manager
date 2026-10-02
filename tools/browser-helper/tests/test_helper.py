import asyncio
import io
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

from browser_helper import core
from browser_helper.__main__ import read_request, write_all

ROOT = Path(__file__).resolve().parents[1]
SECRET = 'never-print-password'
COOKIE_SECRET = 'never-log-cookie'


def cookie(**overrides):
    return dict(name='session', value=COOKIE_SECRET, domain='anyrouter.top',
                path='/', secure=True, httpOnly=True, expires=-1, **overrides)


def response(payload=None, *, url=core.ORIGIN + '/api/user/self', status=200, request_url=None, redirected_from=None):
    return SimpleNamespace(url=url, status=status,
                           request=SimpleNamespace(url=request_url or url, redirected_from=redirected_from,
                                                   method='POST' if url.endswith('/api/user/login') else 'GET', headers={}),
                           json=AsyncMock(return_value=payload if payload is not None else {'success': True, 'data': {'id': 42}}))


class ActionClick(AsyncMock):
    def __call__(self, *args, **kwargs):
        if kwargs.get('trial'):
            return asyncio.sleep(0)  # Trial never dispatches a click event.
        return super().__call__(*args, **kwargs)


class Locator:
    def __init__(self, visible=True, click=None):
        self.first = self
        self.is_visible = AsyncMock(return_value=visible)
        self.is_enabled = AsyncMock(return_value=True)
        self.is_editable = AsyncMock(return_value=True)
        self.fill = AsyncMock()
        self.click = ActionClick(side_effect=click)
        self.count = AsyncMock(return_value=1 if visible else 0)
        self.inner_text = AsyncMock(return_value='Continue')
        self.evaluate = AsyncMock(return_value=True)
        self.element_handle = AsyncMock(return_value=SimpleNamespace(dispose=AsyncMock()))

    def nth(self, index):
        return self


class FakePage:
    def __init__(self, profile=None, *, challenge=False, form=True):
        self.url = core.ORIGIN + '/login'
        self.profile = profile
        self.challenge = challenge
        self.form = form
        self.listeners = {}
        self.main_frame = object()
        self.inputs = {}
        self.goto_urls = []
        self.ready = False
        self.evaluate = AsyncMock(side_effect=lambda script, arg=None: {'error': 'network'} if script == core.SESSION_PROFILE_JS else ({'id': 42 if self.ready else None, 'ready': True} if script == core.USER_STATE_JS else self.challenge))

    async def accepted_login(self):
        self.listeners['response'](response({'success': True}, url=core.ORIGIN + '/api/user/login'))
        self.ready = True
        await asyncio.sleep(0)

    def locator(self, selector):
        if selector not in self.inputs:
            async def click(**kwargs):
                if selector in core.SUBMIT_SELECTORS:
                    await self.accepted_login()
                    self.url = core.ORIGIN + '/console'
                    if self.profile is not None:
                        self.listeners['response'](self.profile)
                        await asyncio.sleep(0)
            self.inputs[selector] = Locator(self.form and selector in core.USERNAME_SELECTORS + core.PASSWORD_SELECTORS + core.SUBMIT_SELECTORS, click)
        return self.inputs[selector]

    def get_by_role(self, *args, **kwargs):
        return Locator(False)

    def set_default_timeout(self, timeout):
        self.default_timeout = timeout

    async def goto(self, url, **kwargs):
        self.goto_urls.append(url)
        self.url = url
        if url.endswith('/console') and self.profile is not None:
            self.listeners['response'](self.profile)
            await asyncio.sleep(0)
        return SimpleNamespace(status=200)

    def on(self, event, callback):
        self.listeners[event] = callback

    def remove_listener(self, event, callback):
        self.listeners.pop(event)


class FakeContext:
    def __init__(self, page, cookies=None):
        self.page = page
        self.cookies = AsyncMock(return_value=[cookie()] if cookies is None else cookies)
        self.new_page = AsyncMock(return_value=page)
        self.route = AsyncMock()
        self.close = AsyncMock()


class FakeBrowser:
    def __init__(self, context):
        self.new_context = AsyncMock(return_value=context)
        self.close = AsyncMock()


class InputTests(unittest.TestCase):
    def test_valid_default_and_cap(self):
        result = core.parse_input(b'{"username":"a","password":"b"}\n')
        self.assertEqual(result.timeout_ms, 60000)
        self.assertEqual(core.parse_input(b'{"username":"a","password":"b","timeout_ms":120000}').timeout_ms, 120000)
        self.assertNotIn('username=', repr(result))
        self.assertNotIn('password=', repr(result))

    def test_invalid_inputs(self):
        invalid = [b'', b'no', b'[]', b'null', b'\xff', b'{}', b'{}\n{}', b'x' * 16385,
                   b'{"username":"a","username":"b","password":"c"}']
        cases = [dict(username='a', password='b', timeout_ms=v) for v in (True, 0, 999, 120001, '60000', 60000.0)]
        cases += [dict(username=v, password='b') for v in ('', ' ', 'a' * 321, 'a\n', 7)]
        cases += [dict(username='a', password=v) for v in ('', 'b' * 4097, 'b\x00', None)]
        cases += [dict(username='a', password='b', url='https://evil.example')]
        for raw in invalid + [json.dumps(v).encode() for v in cases]:
            with self.subTest(raw_length=len(raw)), self.assertRaisesRegex(core.Failure, '^invalid_input$'):
                core.parse_input(raw)

    def test_read_is_bounded(self):
        with self.assertRaises(core.Failure):
            read_request(io.BytesIO(b'x' * 1_000_000))

    def test_no_binary_no_import_or_download(self):
        for value in ('', '/does/not/exist', str(ROOT), 'relative'):
            with self.subTest(value=value), patch.dict(os.environ, {'CLOAKBROWSER_BINARY_PATH': value}), patch.object(core.importlib, 'import_module') as imported:
                with self.assertRaisesRegex(core.Failure, 'browser_unavailable'):
                    core.browser_launcher()
                imported.assert_not_called()

    def test_binary_preserves_environment_and_passes_import(self):
        # Python itself is only a mock executable-path fixture, NOT a browser.
        with patch.dict(os.environ, {'CLOAKBROWSER_BINARY_PATH': sys.executable}), patch.object(core.importlib, 'import_module', return_value=SimpleNamespace(launch_async='mock')):
            self.assertEqual(core.browser_launcher(), 'mock')
            self.assertEqual(os.environ['CLOAKBROWSER_BINARY_PATH'], sys.executable)

    def test_non_executable_binary_no_import(self):
        with patch.dict(os.environ, {'CLOAKBROWSER_BINARY_PATH': str(ROOT / 'pyproject.toml')}), patch.object(core.os, 'access', return_value=False), patch.object(core.importlib, 'import_module') as imported:
            with self.assertRaisesRegex(core.Failure, 'browser_unavailable'):
                core.browser_launcher()
            imported.assert_not_called()


class ValidationTests(unittest.TestCase):
    def test_id_validation(self):
        for value in (True, False, 0, -1, 1.0, '42', None, 2**63):
            self.assertIsNone(core.profile_id({'success': True, 'data': {'id': value}}))
        for value in (1, 42, 2**63 - 1):
            self.assertEqual(core.profile_id({'success': True, 'data': {'id': value}}), str(value))
        self.assertEqual(core.profile_id({'success': True, 'id': 42}), '42')
        self.assertEqual(core.profile_id({'success': True, 'data': {}, 'id': 42}), '42')
        for payload in ({'id': 42}, {'success': False, 'id': 42}, {'success': 1, 'id': 42}, []):
            self.assertIsNone(core.profile_id(payload))

    def test_exact_safe_origin(self):
        for url in ('http://anyrouter.top/console', 'https://anyrouter.top.evil/console',
                    'https://sub.anyrouter.top/console', 'https://anyrouter.top:444/console',
                    'https://user@anyrouter.top/console', 'https://anyrouter.top/console#x',
                    'https://anyrouter.top/console\n', 'https://anyrouter.top:bad/console'):
            self.assertFalse(core.safe_url(url, '/console'))
        self.assertTrue(core.safe_url(core.ORIGIN + ':443/console?x=1', '/console'))

    def test_cookie_schema_and_filter(self):
        good = cookie()
        dotted = dict(good, domain='.anyrouter.top')
        invalid = [dict(good, **overrides) for overrides in (
            {'domain': 'evil.example'}, {'domain': 'sub.anyrouter.top'}, {'domain': '.top'},
            {'name': 'a;b'}, {'name': 'a\rb'}, {'name': ''}, {'value': 'a;b'}, {'value': 'a\nb'},
            {'value': 'a"b'}, {'value': 'a b'}, {'value': 'a\\b'},
            {'expires': float('nan')}, {'expires': True}, {'expires': -2}, {'path': 'x'},
            {'path': '/\n'}, {'secure': 'true'}, {'httpOnly': None})]
        result = core.filtered_cookies([good, dotted] + invalid, core.ORIGIN + '/console')
        self.assertEqual(len(result), 2)
        self.assertEqual(set(result[0]), {'name', 'value', 'domain', 'path', 'secure', 'http_only', 'expires'})
        self.assertEqual(core.filtered_cookies([good], 'https://evil.example/console'), [])
        self.assertEqual(core.filtered_cookies([good], core.ORIGIN + '/login'), [])


class LoginTests(unittest.IsolatedAsyncioTestCase):
    async def execute(self, page, cookies=None, timeout_ms=160):
        self.context = FakeContext(page, cookies)
        self.browser = FakeBrowser(self.context)
        self.launch = AsyncMock(return_value=self.browser)
        with patch.object(core, 'browser_launcher', return_value=self.launch):
            return await core.run_login(core.LoginInput('mock-user', SECRET, timeout_ms))

    async def execute_virtual(self, page, cookies=None, timeout_ms=160):
        # Only selected mock-only cookie/profile fixtures use this clock.
        # Keep wait_for timers and task scheduling real; exclude wall-clock stalls.
        loop = asyncio.get_running_loop()
        now = 1000.0
        original_sleep = asyncio.sleep

        async def sleep(delay, result=None):
            nonlocal now
            if delay > 0:
                now += delay
            return await original_sleep(0, result)

        with patch.object(loop, 'time', lambda: now), patch.object(asyncio, 'sleep', sleep):
            return await self.execute(page, cookies, timeout_ms)

    async def test_success_requires_console_response_same_context(self):
        page = FakePage(response())
        result = await self.execute(page)
        self.assertEqual(result['api_user'], '42')
        self.assertTrue(result['ok'])
        self.launch.assert_awaited_once_with(headless=True)
        self.browser.new_context.assert_awaited_once_with(service_workers='block')
        self.context.cookies.assert_awaited_once_with()
        self.context.close.assert_awaited_once()
        self.browser.close.assert_awaited_once()
        self.assertEqual(page.goto_urls, [core.ORIGIN + '/login'])
        self.assertEqual(page.listeners, {})

    def proactive_page(self, reply=None, after=None, before=None):
        page = FakePage()
        original = page.evaluate.side_effect
        page.proof_routes = []

        async def evaluate(script, arg=None):
            if script != core.SESSION_PROFILE_JS:
                return original(script)
            self.assertEqual(arg['url'], core.ORIGIN + '/api/user/self')
            self.assertEqual(arg['user'], '42')
            self.assertEqual(arg['limit'], core.MAX_PROFILE_BYTES)
            self.assertGreater(arg['timeout'], 0)
            self.assertLess(arg['timeout'], 160)
            request = SimpleNamespace(url=arg['url'], method='GET', redirected_from=None, frame=page.main_frame,
                                      headers={'new-api-user': arg['user'], 'x-helper-proof': arg['proof']})
            route = SimpleNamespace(request=request, continue_=AsyncMock(), fetch=AsyncMock(), abort=AsyncMock())
            if before:
                await before(page, arg, route)
            await self.context.route.await_args.args[1](route)
            route.continue_.assert_awaited_once_with(headers={'new-api-user': '42'})
            route.fetch.assert_not_awaited()
            page.proof_routes.append(route)
            # Neither the correlated event nor a concurrent unsolicited event
            # may invoke the uncapped response.json reader or supply identity.
            correlated = response()
            correlated.request = request
            for event in (correlated, response()):
                event.json.side_effect = AssertionError('uncapped_self_read')
                page.listeners['response'](event)
                await asyncio.sleep(0)
                event.json.assert_not_awaited()
            if after:
                await after(page, arg, route)
            return reply if reply is not None else dict(status=200, type='application/json',
                                                       body='{"success":true,"data":{"id":42}}')

        page.evaluate.side_effect = evaluate
        return page

    async def test_proactive_self_without_spa_one_context_no_reload(self):
        page = self.proactive_page()
        result = await self.execute_virtual(page)
        self.assertTrue(result['ok'])
        self.assertEqual(result['api_user'], '42')
        self.assertTrue(result['cookies'][0]['http_only'])
        self.assertEqual(len(page.proof_routes), 1)
        self.assertEqual(page.goto_urls, [core.ORIGIN + '/login'])
        self.browser.new_context.assert_awaited_once_with(service_workers='block')
        self.context.cookies.assert_awaited_once()
        self.context.close.assert_awaited_once()
        self.browser.close.assert_awaited_once()
        self.assertEqual(page.listeners, {})

    async def test_proactive_marker_cannot_authorize_other_requests(self):
        async def before(page, arg, owned):
            for changes in ({'url': arg['url'] + '?extra=1'}, {'method': 'POST'},
                            {'frame': object()}, {'redirected_from': owned.request},
                            {'headers': {'x-helper-proof': 'wrong', 'new-api-user': '42'}}):
                request = SimpleNamespace(**(vars(owned.request) | changes))
                route = SimpleNamespace(request=request, continue_=AsyncMock(), fetch=AsyncMock(), abort=AsyncMock())
                await self.context.route.await_args.args[1](route)
                route.abort.assert_awaited_once()
                route.continue_.assert_not_awaited()
                route.fetch.assert_not_awaited()
        async def after(page, arg, route):
            route.continue_.reset_mock()
            await self.context.route.await_args.args[1](route)
            route.abort.assert_awaited_once()
            route.continue_.assert_not_awaited()
        self.assertTrue((await self.execute_virtual(self.proactive_page(before=before, after=after)))['ok'])

    async def test_proactive_self_rejection_never_exports_cookies(self):
        for reply in (dict(status=200, type='application/json', body=body) for body in (
                '{"success":true,"data":{"id":43}}', '{"success":false}',
                '{"success":true,"data":{"id":"42"}}', 'malformed-private-body',
                ' ' * (core.MAX_PROFILE_BYTES + 1))):
            page = self.proactive_page(reply)
            result = await self.execute_virtual(page)
            self.assertEqual(result, {'ok': False, 'error': 'user_self_unverified'})
            self.assertEqual(len(page.proof_routes), 1)
            self.context.cookies.assert_not_awaited()
        for reply in (dict(error='oversize'), dict(error='network'),
                      dict(status=302, type='application/json', body='{"success":true,"id":42}')):
            self.assertFalse((await self.execute_virtual(self.proactive_page(reply)))['ok'])
            self.context.cookies.assert_not_awaited()

    async def test_proactive_self_rechecks_console_and_identity(self):
        for change in ('url', 'identity', 'challenge'):
            async def after(page, arg, route):
                if change == 'url':
                    page.url = core.ORIGIN + '/other'
                elif change == 'identity':
                    page.evaluate.side_effect = lambda script: {'id': 43, 'ready': True} if script == core.USER_STATE_JS else False
                else:
                    page.challenge = True
            page = self.proactive_page(after=after)
            self.assertFalse((await self.execute_virtual(page))['ok'])
            self.context.cookies.assert_not_awaited()

    async def test_proactive_self_no_retry_on_transient_or_cancellation(self):
        for mode in ('transient', 'cancel'):
            async def after(page, arg, route):
                if mode == 'transient':
                    from playwright.async_api import Error
                    raise Error('Execution context was destroyed')
                await asyncio.sleep(arg['timeout'] / 1000)
                await asyncio.Future()
            page = self.proactive_page(after=after)
            self.assertFalse((await self.execute_virtual(page))['ok'])
            self.assertEqual(len(page.proof_routes), 1)
            self.context.cookies.assert_not_awaited()
            self.context.close.assert_awaited_once()
            self.browser.close.assert_awaited_once()
            self.assertEqual(page.listeners, {})

    async def test_proactive_self_gates_and_existing_spa_no_duplicate(self):
        for mode in ('unaccepted', 'unstable', 'unsafe', 'existing'):
            page = self.proactive_page()
            if mode == 'unaccepted':
                page.ready = True  # Storage alone cannot authorize a proof fetch.
                page.accepted_login = AsyncMock()
            elif mode == 'unstable':
                original = page.evaluate.side_effect
                count = 0
                async def evaluate(script, arg=None):
                    nonlocal count
                    if script == core.USER_STATE_JS:
                        count += 1
                        return {'id': 42 + count % 2, 'ready': True}
                    return await original(script, arg)
                page.evaluate.side_effect = evaluate
            elif mode == 'unsafe':
                original_login = page.accepted_login
                async def accepted():
                    await original_login()
                    page.challenge = True
                page.accepted_login = accepted
            else:
                page.profile = response()
            result = await self.execute_virtual(page)
            self.assertEqual(result['ok'], mode == 'existing')
            self.assertEqual(page.proof_routes, [])
            if mode != 'existing':
                self.context.cookies.assert_not_awaited()

    async def test_failed_profiles_never_read_cookies(self):
        for profile in (None, response({'success': False, 'data': {'id': 42}}),
                        response(status=401), response({'success': True, 'data': {'id': True}}),
                        response(url='https://evil.example/api/user/self'),
                        response(url=core.ORIGIN + '/other/api/user/self'),
                        response(redirected_from=SimpleNamespace(url='https://evil.example/start', redirected_from=None))):
            with self.subTest(profile=profile is None):
                result = await self.execute(FakePage(profile))
                self.assertEqual(result, {'ok': False, 'error': 'user_self_unverified'})
                self.context.cookies.assert_not_awaited()
                self.browser.close.assert_awaited_once()

    async def test_bad_json_profile(self):
        profile = response()
        profile.json.side_effect = ValueError(SECRET)
        self.assertIsNone(await core.verified_response(profile))

    async def test_console_url_without_profile_never_succeeds(self):
        result = await self.execute(FakePage())
        self.assertFalse(result['ok'])
        self.context.cookies.assert_not_awaited()

    async def test_empty_cookie_is_failure(self):
        result = await self.execute(FakePage(response()), [])
        self.assertEqual(result, {'ok': False, 'error': 'user_self_unverified'})

    async def test_challenge_no_form_fill(self):
        page = FakePage(challenge=True)
        result = await self.execute(page)
        self.assertEqual(result, {'ok': False, 'error': 'challenge_or_block'})
        self.context.cookies.assert_not_awaited()
        self.assertFalse(page.inputs)

    async def test_changed_dom_form_unavailable(self):
        with patch.object(core, 'FORM_WAIT_SECONDS', 0.01):
            result = await self.execute(FakePage(form=False))
        self.assertEqual(result, {'ok': False, 'error': 'login_form_unavailable'})

    async def test_timeout_cleanup(self):
        page = FakePage()
        async def hangs(*args, **kwargs):
            await asyncio.sleep(10)
        page.goto = hangs
        result = await self.execute(page, timeout_ms=10)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        self.context.close.assert_awaited_once()
        self.browser.close.assert_awaited_once()

    async def test_context_failure_closes_browser(self):
        browser = FakeBrowser(None)
        browser.new_context.side_effect = RuntimeError(SECRET)
        with patch.object(core, 'browser_launcher', return_value=AsyncMock(return_value=browser)):
            result = await core.run_login(core.LoginInput('a', SECRET, 1000))
        self.assertEqual(result, {'ok': False, 'error': 'login_failed'})
        browser.close.assert_awaited_once()

    async def test_context_close_error_still_closes_browser(self):
        context, browser = SimpleNamespace(close=AsyncMock(side_effect=RuntimeError(SECRET))), SimpleNamespace(close=AsyncMock())
        await core.close_resources({'context': context, 'browser': browser})
        browser.close.assert_awaited_once()

    async def test_hanging_cleanup_is_bounded(self):
        async def hangs():
            await asyncio.sleep(10)
        context, browser = SimpleNamespace(close=hangs), SimpleNamespace(close=AsyncMock())
        with patch.object(core, 'CLEANUP_SECONDS', 0.02):
            await core.close_resources({'context': context, 'browser': browser})
        browser.close.assert_awaited_once()

    async def test_off_origin_routes_abort(self):
        await self.execute(FakePage(response()))
        guard = self.context.route.await_args.args[1]
        for url in ('https://evil.example/', 'http://anyrouter.top/login'):
            route = SimpleNamespace(request=SimpleNamespace(url=url), abort=AsyncMock(), continue_=AsyncMock())
            await guard(route)
            route.abort.assert_awaited_once()
            route.continue_.assert_not_awaited()

    async def test_redirects_never_fulfilled_or_followed(self):
        await self.execute(FakePage(response()))
        guard = self.context.route.await_args.args[1]
        for status in (301, 302, 303, 307, 308):
            for body in (None, b'username=mock-user&password=secret'):
                request = SimpleNamespace(url=core.ORIGIN + '/api/user/login', post_data_buffer=body, method='POST')
                route = SimpleNamespace(request=request, fetch=AsyncMock(return_value=SimpleNamespace(status=status, url=request.url)),
                                        fulfill=AsyncMock(), abort=AsyncMock(), continue_=AsyncMock())
                await guard(route)
                self.assertEqual(route.fetch.await_args.kwargs['max_redirects'], 0)
                route.abort.assert_awaited_once()
                route.fulfill.assert_not_awaited()
                route.continue_.assert_not_awaited()

    async def test_safe_requests_fetch_no_redirects_and_fulfill(self):
        await self.execute(FakePage(response()))
        guard = self.context.route.await_args.args[1]
        fetched = SimpleNamespace(status=200, url=core.ORIGIN + '/static/app.js')
        route = SimpleNamespace(request=SimpleNamespace(url=fetched.url, post_data_buffer=None), fetch=AsyncMock(return_value=fetched),
                                fulfill=AsyncMock(), abort=AsyncMock(), continue_=AsyncMock())
        await guard(route)
        route.fulfill.assert_awaited_once_with(response=fetched)
        route.abort.assert_not_awaited()
        route.continue_.assert_not_awaited()

    async def test_local_two_origins_redirect_body_never_sent_to_second(self):
        # Real local HTTP endpoints, MOCK route.fetch adapter. This verifies the
        # guard requests no-follow; it does not claim actual Chromium coverage.
        await self.execute(FakePage(response()))
        guard = self.context.route.await_args.args[1]
        first_requests, second_requests = [], []
        redirect_status = 307
        class Second(BaseHTTPRequestHandler):
            def do_POST(self):
                second_requests.append(self.path)
                self.send_response(200)
                self.end_headers()
            def log_message(self, *args):
                pass
        second = ThreadingHTTPServer(('127.0.0.1', 0), Second)
        class First(BaseHTTPRequestHandler):
            def do_POST(self):
                first_requests.append(self.rfile.read(int(self.headers['Content-Length'])))
                self.send_response(redirect_status)
                self.send_header('Location', f'http://127.0.0.1:{second.server_port}/steal')
                self.end_headers()
            def log_message(self, *args):
                pass
        first = ThreadingHTTPServer(('127.0.0.1', 0), First)
        threads = [threading.Thread(target=server.serve_forever, daemon=True) for server in (first, second)]
        for thread in threads:
            thread.start()
        try:
            for status in (307, 308):
                redirect_status = status
                async def fetch(**kwargs):
                    self.assertEqual(kwargs['max_redirects'], 0)
                    def local_fetch():
                        conn = http.client.HTTPConnection('127.0.0.1', first.server_port, timeout=2)
                        try:
                            conn.request('POST', '/login', body=b'mock-credentials')
                            reply = conn.getresponse()
                            reply.read()
                            return SimpleNamespace(status=reply.status, url=core.ORIGIN + '/api/user/login')
                        finally:
                            conn.close()
                    return await asyncio.to_thread(local_fetch)
                route = SimpleNamespace(request=SimpleNamespace(url=core.ORIGIN + '/api/user/login', post_data_buffer=b'mock-credentials'),
                                        fetch=fetch, fulfill=AsyncMock(), abort=AsyncMock(), continue_=AsyncMock())
                await guard(route)
                route.abort.assert_awaited_once()
                route.fulfill.assert_not_awaited()
                route.continue_.assert_not_awaited()
            self.assertEqual(first_requests, [b'mock-credentials', b'mock-credentials'])
            self.assertEqual(second_requests, [])
        finally:
            for server in (first, second):
                await asyncio.to_thread(server.shutdown)
                server.server_close()
            for thread in threads:
                thread.join(timeout=2)

    async def test_api_path_cookie_preserved(self):
        result = await self.execute_virtual(FakePage(response()), [dict(cookie(), path='/api')])
        self.assertTrue(result['ok'])
        self.assertEqual(result['api_user'], '42')
        self.assertEqual(result['cookies'][0]['path'], '/api')
        self.context.cookies.assert_awaited_once_with()
        self.context.close.assert_awaited_once()
        self.browser.close.assert_awaited_once()

    async def test_login_bad_path_before_password_or_click(self):
        for stage in ('entry', 'username', 'password'):
            page = FakePage(response())
            if stage == 'entry':
                original = page.goto
                async def bad_goto(*args, **kwargs):
                    await original(*args, **kwargs)
                    page.url = core.ORIGIN + '/not-login'
                page.goto = bad_goto
            else:
                locator = page.locator(core.USERNAME_SELECTORS[0] if stage == 'username' else core.PASSWORD_SELECTORS[0])
                async def change_path(value, **kwargs):
                    page.url = core.ORIGIN + '/not-login'
                locator.fill.side_effect = change_path
            result = await self.execute(page)
            self.assertEqual(result, {'ok': False, 'error': 'login_form_unavailable'})
            if stage in ('entry', 'username'):
                page.locator(core.PASSWORD_SELECTORS[0]).fill.assert_not_awaited()
            page.locator(core.SUBMIT_SELECTORS[0]).click.assert_not_awaited()

    async def test_early_self_on_login_is_retained_then_console(self):
        page = FakePage()
        async def early_self(**kwargs):
            await page.accepted_login()
            # Synchronous emit during click, while still on /login.
            page.listeners['response'](response())
            await asyncio.sleep(0)
        page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = early_self
        result = await self.execute_virtual(page)
        self.assertTrue(result['ok'])
        self.assertEqual(page.goto_urls, [core.ORIGIN + '/login', core.ORIGIN + '/console'])

    async def test_pre_submission_profile_is_not_accepted(self):
        page = FakePage()
        early_profile = response()
        async def emit_before_fill(value, **kwargs):
            page.listeners['response'](early_profile)
            await asyncio.sleep(0)
        page.locator(core.USERNAME_SELECTORS[0]).fill.side_effect = emit_before_fill
        result = await self.execute_virtual(page)
        self.assertEqual(result, {'ok': False, 'error': 'user_self_unverified'})
        self.context.cookies.assert_not_awaited()
        early_profile.json.assert_not_awaited()
        self.context.close.assert_awaited_once()
        self.browser.close.assert_awaited_once()
        self.assertEqual(page.listeners, {})

    async def test_slow_login_over_five_seconds_not_interrupted(self):
        page = FakePage()
        context = FakeContext(page)
        browser = FakeBrowser(context)
        completed = False
        network_task = None
        async def fetch(**kwargs):
            nonlocal completed
            await asyncio.sleep(5.2)
            completed = True
            return SimpleNamespace(status=200, url=core.ORIGIN + '/api/user/login')
        async def click(**kwargs):
            nonlocal network_task
            guard = context.route.await_args.args[1]
            async def fulfill(**kwargs):
                await page.accepted_login()
            route = SimpleNamespace(request=SimpleNamespace(url=core.ORIGIN + '/api/user/login', post_data_buffer=b'credentials', method='POST'),
                                    fetch=fetch, abort=AsyncMock(), fulfill=fulfill)
            network_task = asyncio.create_task(guard(route))
            await asyncio.sleep(0)
        page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
        original = page.goto
        async def goto(url, **kwargs):
            if url.endswith('/console'):
                self.assertTrue(completed)
                page.profile = response()
            return await original(url, **kwargs)
        page.goto = goto
        with patch.object(core, 'browser_launcher', return_value=AsyncMock(return_value=browser)):
            try:
                result = await core.run_login(core.LoginInput('a', SECRET, 7000))
            finally:
                if network_task is not None:
                    network_task.cancel()
                    await asyncio.gather(network_task, return_exceptions=True)
        self.assertTrue(result['ok'])
        self.assertEqual(page.goto_urls, [core.ORIGIN + '/login', core.ORIGIN + '/console'])

    async def test_passive_transient_challenge_and_http_block(self):
        page = FakePage(response(), challenge=True)
        checks = 0
        async def transient(script):
            nonlocal checks
            if script == core.USER_STATE_JS:
                return {'id': 42 if page.ready else None, 'ready': True}
            checks += 1
            return checks <= 2
        page.evaluate.side_effect = transient
        original = page.goto
        async def goto(url, **kwargs):
            await original(url, **kwargs)
            return SimpleNamespace(status=503)
        page.goto = goto
        result = await self.execute(page, timeout_ms=1000)
        self.assertTrue(result['ok'])
        self.assertGreater(checks, 2)

    async def test_console_transient_challenge_waits_then_returns_profile(self):
        page = FakePage(response())
        console_checks = 0
        async def transient(script):
            nonlocal console_checks
            if script == core.USER_STATE_JS:
                return {'id': 42 if page.ready else None, 'ready': True}
            if page.url.endswith('/console'):
                console_checks += 1
                return console_checks <= 2
            return False
        page.evaluate.side_effect = transient
        result = await self.execute(page, timeout_ms=1000)
        self.assertTrue(result['ok'])
        self.assertGreater(console_checks, 2)

    async def test_unknown_html_at_overall_deadline_is_form_unavailable(self):
        result = await self.execute(FakePage(form=False), timeout_ms=100)
        self.assertEqual(result, {'ok': False, 'error': 'login_form_unavailable'})

    async def test_http_block_waits_until_deadline(self):
        page = FakePage(form=False)
        original = page.goto
        async def goto(url, **kwargs):
            await original(url, **kwargs)
            return SimpleNamespace(status=403)
        page.goto = goto
        result = await self.execute(page, timeout_ms=160)
        self.assertEqual(result, {'ok': False, 'error': 'challenge_or_block'})


class RegressionTests(unittest.IsolatedAsyncioTestCase):
    execute = LoginTests.execute
    async def test_submit_substep_timeout_attribution(self):
        from playwright.async_api import TimeoutError as PlaywrightTimeout
        for action in ('submit_trial', 'page_recheck', 'submit_click'):
            page = FakePage(response())
            submit = page.locator(core.SUBMIT_SELECTORS[0])
            original_click = submit.click.side_effect
            trial_done = False
            async def click(**kwargs):
                nonlocal trial_done
                if kwargs.get('trial'):
                    if action == 'submit_trial':
                        raise PlaywrightTimeout(SECRET)
                    trial_done = True
                    return
                if action == 'submit_click':
                    raise PlaywrightTimeout(SECRET)
                await original_click(**kwargs)
            submit.click = AsyncMock(side_effect=click)
            original_check = core.require_safe_page
            async def check(p, state):
                if action == 'page_recheck' and trial_done:
                    raise PlaywrightTimeout(SECRET)
                return await original_check(p, state)
            with patch.dict(os.environ, {'BROWSER_HELPER_DIAGNOSTICS': '1'}), patch.object(core, 'require_safe_page', check):
                result = await self.execute(page)
            diagnostic = result['diagnostics']
            self.assertEqual(diagnostic['action'], action)
            self.assertEqual(diagnostic['phase'], 'submit')
            self.assertEqual(diagnostic['exception'], 'timeout')
            self.assertTrue(0 <= diagnostic['action_elapsed_ms'] <= 120000)
            if action == 'page_recheck':
                self.assertNotIn('action_timeout_ms', diagnostic)
            else:
                self.assertTrue(0 < diagnostic['action_timeout_ms'] <= 5000)
            self.assertNotIn(SECRET, json.dumps(result))
            self.context.close.assert_awaited_once()

    async def test_non_json_login_error_is_safe_and_challenge_specific(self):
        for challenged in (False, True):
            page = FakePage(response())
            async def click(**kwargs):
                reply = response(url=core.ORIGIN + '/api/user/login')
                reply.json.side_effect = ValueError('private-html-password-token')
                page.listeners['response'](reply)
                page.challenge = challenged
            page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
            with patch.dict(os.environ, {'BROWSER_HELPER_DIAGNOSTICS': '1'}):
                result = await self.execute(page)
            self.assertEqual(result['error'], 'challenge_or_block' if challenged else 'login_failed')
            self.assertFalse(result['diagnostics']['login_json'])
            self.assertNotIn('private-html', json.dumps(result))
            self.context.cookies.assert_not_awaited()

    async def test_key_self_network_failure_is_not_optional(self):
        page = FakePage(response())
        original = page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect
        async def click(**kwargs):
            await original(**kwargs)
            route = SimpleNamespace(request=SimpleNamespace(url=core.ORIGIN + '/api/user/self', method='GET'),
                                    fetch=AsyncMock(side_effect=RuntimeError(SECRET)), abort=AsyncMock())
            await self.context.route.await_args.args[1](route)
        page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
        with patch.dict(os.environ, {'BROWSER_HELPER_DIAGNOSTICS': '1'}):
            result = await self.execute(page)
        self.assertEqual(result['error'], 'user_self_unverified')
        self.assertEqual(result['diagnostics']['failure_request'], 'self')
        self.assertEqual(result['diagnostics']['exception'], 'network')
        self.context.cookies.assert_not_awaited()

    def test_diagnostic_validator_drops_untrusted_values(self):
        dirty = {key: 'private-sentinel' for key in core.diagnostics({})}
        dirty.update(login_status=True, self_status=600, login_success=1, self_success='true', private='private-sentinel')
        clean = core.diagnostics(dirty)
        self.assertEqual(set(clean), set(core.diagnostics({})))
        self.assertNotIn('private-sentinel', json.dumps(clean))
        self.assertIsNone(clean['login_status'])
        self.assertIsNone(clean['self_status'])
        self.assertIsNone(clean['login_success'])

    async def test_http200_login_false_never_reads_cookie_or_navigates(self):
        page = FakePage(response())
        async def click(**kwargs):
            page.listeners['response'](response({'success': False, 'message': SECRET}, url=core.ORIGIN + '/api/user/login'))
            page.listeners['response'](response())
            page.ready = True
        page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
        result = await self.execute(page)
        self.assertEqual(result, {'ok': False, 'error': 'login_failed'})
        self.assertEqual(page.goto_urls, [core.ORIGIN + '/login'])
        self.context.cookies.assert_not_awaited()

    async def test_unrelated_post_completion_does_not_unlock_console(self):
        page = FakePage()
        async def click(**kwargs):
            guard = self.context.route.await_args.args[1]
            route = SimpleNamespace(request=SimpleNamespace(url=core.ORIGIN + '/analytics', method='POST'),
                                    fetch=AsyncMock(return_value=SimpleNamespace(url=core.ORIGIN + '/analytics', status=200)),
                                    fulfill=AsyncMock(), abort=AsyncMock())
            await guard(route)
        page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
        result = await self.execute(page)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        self.assertEqual(page.goto_urls, [core.ORIGIN + '/login'])
        self.context.cookies.assert_not_awaited()

    async def test_delayed_spa_storage_not_interrupted(self):
        page = FakePage(response())
        ready_at = None
        async def click(**kwargs):
            nonlocal ready_at
            page.listeners['response'](response({'success': True}, url=core.ORIGIN + '/api/user/login'))
            ready_at = asyncio.get_running_loop().time() + 0.22
        page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
        async def evaluate(script):
            ready = ready_at is not None and asyncio.get_running_loop().time() >= ready_at
            return {'id': 42 if ready else None, 'ready': True} if script == core.USER_STATE_JS else False
        page.evaluate.side_effect = evaluate
        original = page.goto
        async def goto(url, **kwargs):
            if url.endswith('/console'):
                self.assertGreaterEqual(asyncio.get_running_loop().time(), ready_at)
            return await original(url, **kwargs)
        page.goto = goto
        self.assertTrue((await self.execute(page, timeout_ms=1000))['ok'])
        page.locator(core.SUBMIT_SELECTORS[0]).click.assert_awaited_once()

    async def test_optional_image_redirect_or_network_failure_not_global(self):
        for fails in (False, True):
            page = FakePage(response())
            original = page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect
            async def click(**kwargs):
                guard = self.context.route.await_args.args[1]
                route = SimpleNamespace(request=SimpleNamespace(url=core.ORIGIN + '/optional.png', method='GET'),
                                        fetch=AsyncMock(return_value=SimpleNamespace(status=302, url=core.ORIGIN + '/optional.png'), side_effect=RuntimeError(SECRET) if fails else None),
                                        fulfill=AsyncMock(), abort=AsyncMock())
                await guard(route)
                route.abort.assert_awaited_once()
                await original(**kwargs)
            page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
            self.assertTrue((await self.execute(page))['ok'])

    async def test_playwright_timeout_fill_submit_and_profile(self):
        from playwright.async_api import TimeoutError as PlaywrightTimeout
        for phase in ('fill', 'submit', 'profile_wait'):
            page = FakePage(response())
            if phase == 'fill':
                page.locator(core.USERNAME_SELECTORS[0]).fill.side_effect = PlaywrightTimeout(SECRET)
            elif phase == 'submit':
                page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = PlaywrightTimeout(SECRET)
            else:
                original = page.evaluate.side_effect
                def evaluate(script):
                    if page.ready:
                        raise PlaywrightTimeout(SECRET)
                    return original(script)
                page.evaluate.side_effect = evaluate
            with patch.dict(os.environ, {'BROWSER_HELPER_DIAGNOSTICS': '1'}):
                result = await self.execute(page)
            self.assertEqual(result['error'], 'user_self_unverified' if phase == 'profile_wait' else 'timeout')
            self.assertEqual(result['diagnostics']['phase'], phase)
            self.assertEqual(result['diagnostics']['exception'], 'timeout')
            if phase == 'submit':
                self.assertEqual(result['diagnostics']['action'], 'submit_click')
            else:
                self.assertNotIn('action', result['diagnostics'])
            self.context.close.assert_awaited_once()
            self.assertLessEqual(page.locator(core.SUBMIT_SELECTORS[0]).click.await_count, 1)

    async def test_execution_context_navigation_retry_is_bounded(self):
        from playwright.async_api import Error
        for persistent in (False, True):
            page = FakePage(response())
            original = page.evaluate.side_effect
            count = 0
            def evaluate(script):
                nonlocal count
                if page.ready:
                    count += 1
                    if persistent or count == 1:
                        raise Error('Execution context was destroyed, most likely because of a navigation.')
                return original(script)
            page.evaluate.side_effect = evaluate
            result = await self.execute(page, timeout_ms=1000)
            self.assertEqual(result['ok'], not persistent)
            if persistent:
                self.assertEqual(result['error'], 'user_self_unverified')
                self.assertEqual(count, 3)
            page.locator(core.SUBMIT_SELECTORS[0]).click.assert_awaited_once()

    async def test_disabled_form_controls_never_submit(self):
        page = FakePage(response())
        for selector in core.SUBMIT_SELECTORS:
            page.locator(selector).is_enabled.return_value = False
        result = await self.execute(page)
        self.assertEqual(result['error'], 'login_form_unavailable')
        page.locator(core.SUBMIT_SELECTORS[0]).click.assert_not_awaited()

    async def test_storage_self_id_mismatch(self):
        page = FakePage(response({'success': True, 'data': {'id': 43}}))
        result = await self.execute(page)
        self.assertEqual(result['error'], 'user_self_unverified')
        self.context.cookies.assert_not_awaited()


class ProtocolTests(unittest.TestCase):
    def test_diagnostics_opt_in_fixed_and_secret_free(self):
        sentinels = ['private-user-sentinel', 'private-password-sentinel', 'private-token-sentinel',
                     'private-body-sentinel', 'private-query-sentinel', 'private-field-sentinel']
        script = '''
import asyncio, os, runpy, sys
sys.path.insert(0, 'tests')
from test_helper import FakePage, FakeContext, FakeBrowser, response
from browser_helper import core
page = FakePage(response())
async def rejected(**kwargs):
    page.listeners['response'](response({'success':False,'message':'private-body-sentinel', 'token':'private-token-sentinel','private':'private-field-sentinel'},url=core.ORIGIN+'/api/user/login'))
    page.url = core.ORIGIN+'/login?private-query-sentinel'
core.browser_launcher = lambda: (lambda **kwargs: asyncio.sleep(0, result=FakeBrowser(FakeContext(page))))
page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = rejected
sys.modules.pop('browser_helper.__main__', None)
runpy.run_module('browser_helper',run_name='__main__')
'''
        for value in ('', 'true', '0', '1'):
            env = dict(os.environ, BROWSER_HELPER_DIAGNOSTICS=value)
            process = subprocess.run([sys.executable, '-B', '-c', script], input=b'{"username":"private-user-sentinel","password":"private-password-sentinel","timeout_ms":1000}\n', capture_output=True, cwd=ROOT, env=env, timeout=10)
            result = json.loads(process.stdout)
            self.assertEqual(process.stderr, b'')
            self.assertEqual(process.stdout.count(b'\n'), 1)
            self.assertEqual(result['error'], 'login_failed')
            for sentinel in sentinels:
                self.assertNotIn(sentinel.encode(), process.stdout + process.stderr)
            self.assertEqual('diagnostics' in result, value == '1')
            if value == '1':
                self.assertEqual(result['diagnostics'], core.diagnostics(dict(phase='profile_wait', page='login', login_requested=True, login_status=200, login_json=True, login_success=False)))

    def test_subprocess_json_only_and_no_binary(self):
        env = dict(os.environ, CLOAKBROWSER_BINARY_PATH='', PYTHONDONTWRITEBYTECODE='1')
        for raw, code in ((b'not-json\n', 'invalid_input'), (b'x' * 20000, 'invalid_input'),
                          (json.dumps({'username': 'mock-user', 'password': SECRET}).encode() + b'\n', 'browser_unavailable')):
            process = subprocess.run([sys.executable, '-m', 'browser_helper'], input=raw,
                                     capture_output=True, cwd=ROOT, env=env, timeout=10)
            self.assertEqual(json.loads(process.stdout), {'ok': False, 'error': code})
            self.assertEqual(process.stdout.count(b'\n'), 1)
            self.assertEqual(process.stderr, b'')
            self.assertNotIn(SECRET.encode(), process.stdout)
            self.assertEqual(process.returncode, 1)

    def test_native_buffers_and_exit_hooks_stay_discarded(self):
        # Probe the real process entry, not fd mutations in the test process.
        for success in (True, False):
            script = '''
import atexit, ctypes, os, runpy, sys
from browser_helper import core
libc = ctypes.CDLL(None)
async def noisy(credentials, state=None):
    print("never-print-password")
    print("never-log-cookie", file=sys.stderr)
    os.write(1, b"never-log-cookie")
    os.write(2, b"never-print-password")
    libc.printf(b"never-print-password")
    def late():
        libc.fflush(None)
        libc.printf(b"never-log-cookie")
        os.write(1, b"never-print-password")
        os.write(2, b"never-log-cookie")
    atexit.register(late)
    if SUCCESS:
        return {"ok": True, "cookies": [{"name": "session", "value": "never-log-cookie", "domain": "anyrouter.top", "path": "/api", "secure": True, "http_only": True, "expires": -1}], "api_user": "42"}
    raise RuntimeError("never-print-password never-log-cookie")
core.run_login = noisy
runpy.run_module("browser_helper", run_name="__main__")
'''.replace('SUCCESS', str(success))
            process = subprocess.run([sys.executable, '-B', '-c', script], input=b'{"username":"a","password":"b"}\n', capture_output=True, cwd=ROOT, timeout=10)
            result = json.loads(process.stdout)
            self.assertEqual(result['ok'], success)
            self.assertEqual(process.returncode, 0 if success else 1)
            self.assertEqual(process.stdout.count(b'\n'), 1)
            self.assertNotIn(SECRET.encode(), process.stdout)
            self.assertEqual(process.stderr, b'')
            if success:
                self.assertEqual(result['cookies'][0]['value'], COOKIE_SECRET)
            else:
                self.assertEqual(result, {'ok': False, 'error': 'login_failed'})

    def test_protocol_write_all_partial_and_interrupted(self):
        chunks = []
        def partial(fd, data):
            chunks.append(bytes(data[:2]))
            return min(2, len(data))
        with patch('browser_helper.__main__.os.write', side_effect=[InterruptedError(), 2, 1]) as write:
            write_all(123, b'abc')
            self.assertEqual(bytes(write.call_args.args[1]), b'c')
        with patch('browser_helper.__main__.os.write', side_effect=partial):
            write_all(123, b'abcdefg')
        self.assertEqual(b''.join(chunks), b'abcdefg')

    def test_process_watchdog_single_safe_result_when_loop_or_teardown_blocks(self):
        # Real module entry, no browser/network. Blocking native-style startup
        # cannot be interrupted by asyncio.wait_for; neither can stuck teardown.
        for enabled, phase in ((False, 'launch'), (True, 'profile_wait'), (True, 'done')):
            script = '''
import asyncio, runpy, time
from browser_helper import core
core.CLEANUP_SECONDS = 0.1
async def blocked(credentials, state):
    state.update(phase=PHASE, login_requested=True, login_status=200,
                 login_success=True, self_requested=True)
    if PHASE == 'done':
        asyncio.get_running_loop().call_soon(time.sleep, 20)
        return {'ok':True}
    time.sleep(20)
core.run_login = blocked
runpy.run_module('browser_helper', run_name='__main__')
'''.replace('PHASE', repr(phase))
            process = subprocess.run([sys.executable, '-B', '-c', script],
                                     input=b'{"username":"private-user-sentinel","password":"private-password-sentinel","timeout_ms":1000}\n',
                                     capture_output=True, cwd=ROOT,
                                     env=dict(os.environ, BROWSER_HELPER_DIAGNOSTICS='1' if enabled else '0'), timeout=5)
            result = json.loads(process.stdout)
            self.assertEqual(process.returncode, 1)
            self.assertEqual(process.stdout.count(b'\n'), 1)
            self.assertEqual(process.stderr, b'')
            self.assertEqual(result['error'], 'timeout')
            self.assertNotIn(b'sentinel', process.stdout)
            self.assertEqual('diagnostics' in result, enabled)
            if enabled:
                self.assertEqual(result['diagnostics'], core.diagnostics(dict(
                    phase=phase, exception='timeout', login_requested=True,
                    login_status=200, login_success=True, self_requested=True)))

    def test_incomplete_stdin_is_bounded(self):
        process = subprocess.Popen([sys.executable, '-B', '-m', 'browser_helper'],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, cwd=ROOT)
        try:
            process.stdin.write(b'{"username":')
            process.stdin.flush()
            process.wait(timeout=8)
            self.assertEqual(json.loads(process.stdout.read()), {'ok': False, 'error': 'invalid_input'})
            self.assertEqual(process.stderr.read(), b'')
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stdin.close()
            process.stdout.close()
            process.stderr.close()


if __name__ == '__main__':
    unittest.main()
