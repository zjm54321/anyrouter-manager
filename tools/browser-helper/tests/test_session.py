import asyncio
import json
import os
import subprocess
import sys
import time
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

from browser_helper import core
from test_helper import ROOT, COOKIE_SECRET, cookie, FakeContext, FakeBrowser


def session_input(**overrides):
    item = cookie()
    item['http_only'] = item.pop('httpOnly')
    return dict(mode='session_verify', cookies=[item], api_user='42', timeout_ms=1000, **overrides)


def reply(status=200, payload=None, content_type='application/json'):
    return dict(status=status, type=content_type,
                body=json.dumps({'success': True, 'data': {'id': 42}} if payload is None else payload))


class SessionInputTests(unittest.TestCase):
    def parse(self, data):
        return core.parse_input(json.dumps(data).encode())

    def test_login_explicit_default_compatible(self):
        self.assertIsInstance(self.parse(dict(mode='login', username='a', password='b')), core.LoginInput)

    def test_session_exact_schema_and_native_cookie(self):
        value = self.parse(session_input())
        self.assertIsInstance(value, core.SessionInput)
        self.assertEqual(value.cookies[0]['httpOnly'], True)
        self.assertNotIn(COOKIE_SECRET, repr(value))
        self.assertEqual(core.MAX_INPUT_BYTES, 65536)

    def test_invalid_mode_keys_and_id(self):
        base = session_input()
        cases = [dict(base, mode=v) for v in ('unknown', None, True, {}, [])]
        cases += [dict(base, **{key: 'private-sentinel'}) for key in ('username', 'password', 'token', 'url')]
        cases += [dict(base, api_user=v) for v in (42, True, '', '0', '-1', '+1', '1.0', '４２', str(2**63), '1'*20)]
        cases += [{k: v for k, v in base.items() if k != key} for key in base]
        for item in cases:
            with self.subTest(keys=list(item)), self.assertRaisesRegex(core.Failure, '^invalid_input$'):
                self.parse(item)

    def test_cookie_schema_and_size_bounds(self):
        base = session_input()
        good = base['cookies'][0]
        invalid = [None, {}, [], [good]*129, [dict(good, value='a'*32768)],
                   [dict(good, value='a'*300)]*128]
        invalid += [[dict(good, **change)] for change in (
            {'domain':'evil.example'}, {'domain':'sub.anyrouter.top'}, {'domain':'.top'},
            {'name':'a;b'}, {'value':'a\nb'}, {'value':'a b'}, {'path':'bad'}, {'path':'/\x00'},
            {'expires':time.time()-1}, {'expires':0}, {'expires':-0.5}, {'expires':float('nan')},
            {'expires':float('inf')}, {'expires':10**400},
            {'expires':True}, {'secure':1}, {'http_only':'true'}, {'token':'private-sentinel'})]
        invalid += [[{k:v for k,v in good.items() if k != key}] for key in good]
        for cookies in invalid:
            with self.subTest(count=len(cookies) if isinstance(cookies, list) else None), self.assertRaises(core.Failure):
                self.parse(dict(base, cookies=cookies))
        self.assertIsInstance(self.parse(dict(base, cookies=[dict(good, value='a'*20000)])), core.SessionInput)

    def test_expired_output_cookies_filtered(self):
        cookies = [cookie(), dict(cookie(), expires=time.time()-1), dict(cookie(), expires=time.time()+100)]
        self.assertEqual(len(core.filtered_cookies(cookies, core.ORIGIN+'/console')), 2)

    def test_strict_session_url_authority_and_paths(self):
        for suffix in (':443/console', '/%63onsole', '/api/%2fuser', '/a/../console', '//console', '/a\\b', '/console#x', '/console#'):
            self.assertFalse(core.safe_session_url(core.ORIGIN+suffix))
        self.assertTrue(core.safe_session_url(core.ORIGIN+'/api/user/self'))


class SessionPage:
    def __init__(self, profile=None, challenge=False, target='/console'):
        self.url = core.ORIGIN+'/console'
        self.target = target
        self.challenge = challenge
        self.profile = reply() if profile is None else profile
        self.evaluate = AsyncMock(side_effect=self.evaluation)
        self.goto = AsyncMock(side_effect=self.navigation)
        self.locator = AsyncMock(side_effect=AssertionError('must_not_fill_or_click'))
        self.get_by_role = self.locator
        self.main_frame = object()

    def set_default_timeout(self, timeout):
        pass

    async def navigation(self, url, **kwargs):
        self.url = core.ORIGIN+self.target
        return SimpleNamespace(status=200)

    async def evaluation(self, script, arg=None):
        if script == core.CHALLENGE_JS:
            return self.challenge
        assert script == core.SESSION_PROFILE_JS
        assert arg['url'] == core.ORIGIN+'/api/user/self'
        assert arg['user'] == '42'
        assert arg['limit'] == 262144
        return self.profile


class SessionTests(unittest.IsolatedAsyncioTestCase):
    async def execute(self, page=None, cookies=None, timeout=180):
        self.page = page or SessionPage()
        self.context = FakeContext(self.page, cookies)
        self.context.add_cookies = AsyncMock()
        self.context.add_init_script = AsyncMock()
        self.context.route_web_socket = AsyncMock()
        self.browser = FakeBrowser(self.context)
        self.launch = AsyncMock(return_value=self.browser)
        native = [cookie()]
        with patch.object(core, 'browser_launcher', return_value=self.launch):
            return await core.run_login(core.SessionInput(native, '42', timeout))

    async def test_success_one_context_cookie_import_no_login(self):
        result = await self.execute(cookies=[dict(cookie(), path='/api')])
        self.assertTrue(result['ok'])
        self.assertEqual(result['api_user'], '42')
        self.assertEqual(result['cookies'][0]['path'], '/api')
        self.launch.assert_awaited_once_with(headless=True)
        self.browser.new_context.assert_awaited_once_with(service_workers='block')
        self.context.add_cookies.assert_awaited_once_with([cookie()])
        self.context.new_page.assert_awaited_once()
        self.page.locator.assert_not_called()
        self.assertEqual(self.page.goto.await_args.args, (core.ORIGIN+'/console',))
        self.context.close.assert_awaited_once()
        self.browser.close.assert_awaited_once()

    async def test_expired_json_and_login_redirect(self):
        for profile in (reply(401, {'message':'private-sentinel'}), reply(403, {'success':False}), reply(200, {'success':False})):
            self.assertEqual(await self.execute(SessionPage(profile)), {'ok':False, 'error':'session_expired'})
            self.context.cookies.assert_not_awaited()
        self.assertEqual((await self.execute(SessionPage(target='/login')))['error'], 'session_expired')

    async def test_mismatch_or_invalid_self_never_returns_cookie(self):
        for profile in (reply(payload={'success':True, 'data':{'id':43}}), reply(payload={'success':True}),
                        reply(payload={'success':True, 'id':'42'}), dict(error='oversize'), dict(error='network'),
                        reply(200, 'private-sentinel', 'text/html')):
            self.assertEqual((await self.execute(SessionPage(profile)))['error'], 'user_self_unverified')
            self.context.cookies.assert_not_awaited()

    async def test_html_challenge_waits_and_does_not_claim_expiry(self):
        for status in (401, 403, 429, 503):
            result = await self.execute(SessionPage(reply(status, 'private-sentinel', 'text/html')))
            self.assertEqual(result['error'], 'challenge_or_block')
            self.context.cookies.assert_not_awaited()
        self.assertEqual((await self.execute(SessionPage(challenge=True)))['error'], 'challenge_or_block')

    async def test_transient_challenge_passively_resolves(self):
        page = SessionPage(challenge=True)
        original = page.evaluation
        async def evaluate(script, arg=None):
            result = await original(script, arg)
            page.challenge = False
            return result
        page.evaluate.side_effect = evaluate
        self.assertTrue((await self.execute(page, timeout=1000))['ok'])
        page.locator.assert_not_called()

    async def test_all_writes_and_unsafe_origins_abort_before_fetch(self):
        await self.execute()
        guard = self.context.route.await_args.args[1]
        for method in ('POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS', 'CONNECT'):
            for path in ('/api/user/sign_in', '/api/user/login', '/api/user/self', '/307', '/308'):
                route = SimpleNamespace(request=SimpleNamespace(url=core.ORIGIN+path, method=method),
                                        abort=AsyncMock(), fetch=AsyncMock(), continue_=AsyncMock())
                await guard(route)
                route.abort.assert_awaited_once()
                route.fetch.assert_not_awaited()
                route.continue_.assert_not_awaited()
        for url in ('https://evil.example/', core.ORIGIN+':443/api/user/self', core.ORIGIN+'/api/%75ser/self'):
            route = SimpleNamespace(request=SimpleNamespace(url=url, method='GET'), abort=AsyncMock(), fetch=AsyncMock())
            await guard(route)
            route.fetch.assert_not_awaited()

    async def test_get_redirects_abort_no_follow(self):
        await self.execute()
        guard = self.context.route.await_args.args[1]
        for status in (301, 302, 303, 307, 308):
            request = SimpleNamespace(url=core.ORIGIN+'/optional', method='GET')
            route = SimpleNamespace(request=request, fetch=AsyncMock(return_value=SimpleNamespace(status=status, url=request.url, headers={})),
                                    abort=AsyncMock(), fulfill=AsyncMock(), continue_=AsyncMock())
            await guard(route)
            self.assertEqual(route.fetch.await_args.kwargs['max_redirects'], 0)
            route.abort.assert_awaited_once()
            route.fulfill.assert_not_awaited()

    async def test_self_stream_private_marker_removed(self):
        await self.execute()
        proof = self.page.evaluate.await_args.args[1]['proof']
        request = SimpleNamespace(url=core.ORIGIN+'/api/user/self', method='GET',
                                  headers={'new-api-user':'42', 'x-helper-proof':proof})
        route = SimpleNamespace(request=request, abort=AsyncMock(), fetch=AsyncMock(), continue_=AsyncMock())
        await self.context.route.await_args.args[1](route)
        route.continue_.assert_awaited_once_with(headers={'new-api-user':'42'})
        route.fetch.assert_not_awaited()

    async def test_websockets_rejected_without_connect(self):
        await self.execute()
        socket = SimpleNamespace(close=AsyncMock(), connect_to_server=AsyncMock())
        await self.context.route_web_socket.await_args.args[1](socket)
        socket.close.assert_awaited_once()
        socket.connect_to_server.assert_not_awaited()

    async def test_client_login_navigation_aborted_before_fetch(self):
        await self.execute()
        request = SimpleNamespace(url=core.ORIGIN+'/login', method='GET', is_navigation_request=lambda: True)
        route = SimpleNamespace(request=request, abort=AsyncMock(), fetch=AsyncMock())
        await self.context.route.await_args.args[1](route)
        route.abort.assert_awaited_once()
        route.fetch.assert_not_awaited()

    async def test_navigation_transient_profile_evaluation_retried(self):
        page = SessionPage()
        original = page.evaluation
        first = True
        async def evaluate(script, arg=None):
            nonlocal first
            if script == core.SESSION_PROFILE_JS and first:
                first = False
                from playwright.async_api import Error
                raise Error('Execution context was destroyed, most likely because of a navigation')
            return await original(script, arg)
        page.evaluate.side_effect = evaluate
        self.assertTrue((await self.execute(page, timeout=1000))['ok'])

    def test_signed_i64_profile_identity_not_js_rounded(self):
        expected = str(2**63-1)
        self.assertEqual(core.session_profile(reply(payload={'success':True, 'data':{'id':int(expected)}}), expected, {}), expected)

    async def test_failure_diagnostics_fixed_secret_free(self):
        with patch.dict(os.environ, {'BROWSER_HELPER_DIAGNOSTICS':'1'}):
            result = await self.execute(SessionPage(reply(401, {'message':COOKIE_SECRET, 'token':'private-token'})))
        self.assertEqual(set(result['diagnostics']), set(core.diagnostics({})))
        self.assertNotIn(COOKIE_SECRET, json.dumps(result))
        self.assertNotIn('private-token', json.dumps(result))


class SessionProtocolTests(unittest.TestCase):
    def test_real_entry_session_failure_secret_suppression(self):
        script = '''
import asyncio, runpy, sys
sys.path.insert(0, 'tests')
from test_session import SessionPage, reply
from test_helper import FakeContext, FakeBrowser
from unittest.mock import AsyncMock
from browser_helper import core
context = FakeContext(SessionPage(reply(401, {'message':'private-body-sentinel'})))
context.add_cookies = AsyncMock(); context.add_init_script = AsyncMock(); context.route_web_socket = AsyncMock()
core.browser_launcher = lambda: AsyncMock(return_value=FakeBrowser(context))
sys.modules.pop('browser_helper.__main__', None)
runpy.run_module('browser_helper', run_name='__main__')
'''
        for opt in ('0', '1'):
            process = subprocess.run([sys.executable, '-B', '-c', script], input=json.dumps(session_input()).encode()+b'\n',
                                     capture_output=True, cwd=ROOT, env=dict(os.environ, BROWSER_HELPER_DIAGNOSTICS=opt), timeout=10)
            result = json.loads(process.stdout)
            self.assertEqual(result['error'], 'session_expired')
            self.assertEqual(process.stderr, b'')
            self.assertEqual(process.stdout.count(b'\n'), 1)
            self.assertNotIn(COOKIE_SECRET.encode(), process.stdout)
            self.assertNotIn(b'private-body-sentinel', process.stdout)
            self.assertEqual('diagnostics' in result, opt == '1')
