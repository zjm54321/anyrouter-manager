import asyncio
import json
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

from browser_helper import core
import test_helper
from test_helper import FakePage, Locator, mock_clock, response


class Group:
    def __init__(self, items):
        self.items = items
        self.count = AsyncMock(return_value=len(items))

    def nth(self, index):
        return self.items[index]


class LoginPolicyTests(unittest.IsolatedAsyncioTestCase):
    execute = test_helper.LoginTests.execute

    def test_notice_real_js_exact_titles_and_security_exclusions(self):
        # Run the production predicate, not a mocked evaluate(True). Playwright
        # already supplies Node; the Nix shell overrides its executable path.
        from playwright._impl._driver import compute_driver_executable

        fixtures = [({'title': title}, True) for title in
                    ('公告', '通知', 'Notice', 'System Notice', 'Announcement', 'notice', '系统公告')]
        fixtures += [({'title': title}, False) for title in
                     ('系统通知', '系统公告说明', '重要系统公告', '公告验证', 'Unknown', '',
                      'System Notification', 'System Notice Extra', 'Important System Notice')]
        fixtures += [({'body': word}, False) for word in
                     ('验证码', '验证', '协议', '条款', '同意', 'captcha', 'verify',
                      'agreement', 'terms', 'consent')]
        fixtures += [({'control': control}, False) for control in
                     ('input', 'textarea', 'select', 'form', 'iframe',
                      '[contenteditable="true"]', '.nc-container', '#nocaptcha',
                      '.cf-turnstile', '.g-recaptcha')]
        fixtures += [({'title_count': count}, False) for count in (0, 2)]
        fixtures += [({'close_count': count}, False) for count in (0, 2)]
        fixtures += [({'close_form': True}, False), ({'close_type': 'submit'}, False)]
        fixtures += [({'body': '登录后可以查看服务通知 / Registration and login information'}, True)]
        # Apply the same exclusions to the newly observed English title.
        fixtures += [(dict(fixture, title='System Notice'), expected)
                     for fixture, expected in fixtures.copy() if 'title' not in fixture]
        script = r"""
const {predicate, fixtures} = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
const eligible = eval('(' + predicate + ')');
const results = fixtures.map(f => {
    const title = f.title ?? '系统公告';
    const d = {
        innerText: title + '\n' + (f.body ?? ''),
        querySelectorAll(selector) {
            if (selector === '.semi-modal-title')
                return Array.from({length: f.title_count ?? 1}, () => ({innerText: title}));
            if (selector === 'button.semi-modal-close:has(.semi-icon-close)')
                return Array.from({length: f.close_count ?? 1}, () => ({
                    form: f.close_form ? {} : null, type: f.close_type ?? 'button'
                }));
            throw new Error('unexpected fixture selector');
        },
        querySelector(selector) {
            return selector.split(',').includes(f.control) ? {} : null;
        }
    };
    return eligible(d);
});
process.stdout.write(JSON.stringify(results));
"""
        node, _ = compute_driver_executable()
        process = subprocess.run(
            [node, '-e', script],
            input=json.dumps({'predicate': core.NOTICE_ELIGIBLE_JS,
                              'fixtures': [fixture for fixture, _ in fixtures]}),
            text=True, capture_output=True, check=True, timeout=5,
        )
        self.assertEqual(process.stderr, '')
        results = json.loads(process.stdout)
        self.assertEqual(len(results), len(fixtures))
        for (fixture, expected), actual in zip(fixtures, results):
            with self.subTest(fixture=fixture):
                self.assertIs(actual, expected)

    @mock_clock
    async def test_auto_signin_queries_and_write_methods_never_fetch_or_poison_login(self):
        page = FakePage(response())
        original = page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect
        async def click(**kwargs):
            await original(**kwargs)
            guard = self.context.route.await_args.args[1]
            for method in ('POST', 'PUT', 'PATCH', 'DELETE'):
                for query in ('', '?x=1', '?return=%2Fconsole&token=private-query-sentinel'):
                    route = SimpleNamespace(request=SimpleNamespace(url=core.ORIGIN+'/api/user/sign_in'+query, method=method),
                                            fetch=AsyncMock(), fulfill=AsyncMock(), abort=AsyncMock())
                    await guard(route)
                    route.abort.assert_awaited_once()
                    route.fetch.assert_not_awaited()
                    route.fulfill.assert_not_awaited()
        page.locator(core.SUBMIT_SELECTORS[0]).click.side_effect = click
        result = await self.execute(page, timeout_ms=1000)
        self.assertTrue(result['ok'], result.get('error'))
        self.assertEqual(result['api_user'], '42')
        page.locator(core.SUBMIT_SELECTORS[0]).click.assert_awaited_once()

    @mock_clock
    async def test_login_query_is_preserved_not_stripped(self):
        url = core.ORIGIN+'/api/user/login?turnstile=private-query-sentinel&x=%2F'
        page = FakePage(response())
        page.login_event.url = page.login_event.request.url = url
        page.login_event.request.redirected_from = None
        self.assertEqual(core.request_kind(page.login_event.request), 'login')
        result = await self.execute(page)
        self.assertTrue(result['ok'], result.get('error'))
        route, reply = page.login_route, page.login_fetched
        self.assertIs(route.request, page.login_event.request)
        self.assertEqual(route.request.url, url)
        self.assertEqual(reply.url, url)
        self.assertEqual(route.fetch.await_args.kwargs['max_redirects'], 0)
        route.fulfill.assert_awaited_once_with(response=reply)
        reply.body.assert_awaited_once()
        reply.dispose.assert_awaited_once()
        page.login_event.json.assert_not_awaited()

    async def test_continue_selected_after_wrong_owner_header(self):
        username, password = Locator(), Locator()
        header, wrong, actual = Locator(), Locator(), Locator()
        header.inner_text.return_value = 'Sign in'
        for button in (header, wrong):
            button.evaluate.return_value = False
        actual.inner_text.return_value = 'Continue'
        page = SimpleNamespace(locator=lambda _: Group([header, wrong, actual]), get_by_role=lambda *a, **k: Group([]))
        self.assertIs(await core.owned_submit(page, username, password), actual)
        for item in (username, password):
            item.element_handle.return_value.dispose.assert_awaited_once()
        for item in (header, wrong, actual):
            item.click.assert_not_awaited()

    async def test_missing_or_ambiguous_owner_never_selects_submit(self):
        button = Locator()
        button.evaluate.return_value = False
        page = SimpleNamespace(locator=lambda _: Group([button]), get_by_role=lambda *a, **k: Group([button]))
        self.assertIsNone(await core.owned_submit(page, Locator(), Locator()))
        self.assertIsNone(await core.owned_submit(page, None, Locator()))

    def notice_fixture(self, eligible=True, count=1):
        close = Locator()
        dialog = Locator()
        dialog.evaluate.return_value = eligible
        dialog.locator = lambda _: close
        dialog.wait_for = AsyncMock()
        page = FakePage()
        page.locator = lambda _: Group([dialog]*count)
        state = dict(deadline=asyncio.get_running_loop().time()+1, submitted=False)
        return page, state, close, dialog

    async def test_notice_close_is_once_before_submit_trial(self):
        page, state, close, dialog = self.notice_fixture()
        self.assertTrue(await core.dismiss_notice_once(page, state))
        self.assertFalse(await core.dismiss_notice_once(page, state))
        close.click.assert_awaited_once()
        self.assertNotIn('force', close.click.await_args.kwargs)
        dialog.wait_for.assert_awaited_once()

    async def test_unknown_multiple_and_challenge_dialogs_not_dismissed(self):
        for eligible, count, challenge in ((False, 1, False), (True, 2, False), (True, 1, True), (True, 0, False)):
            page, state, close, _ = self.notice_fixture(eligible, count)
            page.challenge = challenge
            self.assertFalse(await core.dismiss_notice_once(page, state))
            close.click.assert_not_awaited()

    async def test_notice_timeout_does_not_retry_close(self):
        page, state, close, _ = self.notice_fixture()
        close.click.side_effect = TimeoutError('private-error-sentinel')
        with self.assertRaises(TimeoutError):
            await core.dismiss_notice_once(page, state)
        self.assertFalse(await core.dismiss_notice_once(page, state))
        close.click.assert_awaited_once()

    @mock_clock
    async def test_failed_trial_never_submits_credentials(self):
        page = FakePage(response())
        click = AsyncMock(side_effect=TimeoutError('private-error-sentinel'))
        page.locator(core.SUBMIT_SELECTORS[0]).click = click
        result = await self.execute(page)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        click.assert_awaited_once()
        self.assertTrue(click.await_args.kwargs['trial'])
        self.context.cookies.assert_not_awaited()
