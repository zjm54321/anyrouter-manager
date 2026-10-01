import asyncio
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

from browser_helper import core
import test_helper
from test_helper import FakePage, Locator, response


class Group:
    def __init__(self, items):
        self.items = items
        self.count = AsyncMock(return_value=len(items))

    def nth(self, index):
        return self.items[index]


class LoginPolicyTests(unittest.IsolatedAsyncioTestCase):
    execute = test_helper.LoginTests.execute

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
        self.assertTrue(result['ok'])
        self.assertEqual(result['api_user'], '42')
        page.locator(core.SUBMIT_SELECTORS[0]).click.assert_awaited_once()

    async def test_login_query_is_preserved_not_stripped(self):
        await self.execute(FakePage(response()))
        url = core.ORIGIN+'/api/user/login?turnstile=private-query-sentinel&x=%2F'
        request = SimpleNamespace(url=url, method='POST')
        self.assertEqual(core.request_kind(request), 'login')
        reply = SimpleNamespace(url=url, status=200)
        route = SimpleNamespace(request=request, fetch=AsyncMock(return_value=reply), abort=AsyncMock(), fulfill=AsyncMock())
        await self.context.route.await_args.args[1](route)
        self.assertEqual(route.request.url, url)
        self.assertEqual(route.fetch.await_args.kwargs['max_redirects'], 0)
        route.fulfill.assert_awaited_once_with(response=reply)

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

    async def test_failed_trial_never_submits_credentials(self):
        page = FakePage(response())
        click = AsyncMock(side_effect=TimeoutError('private-error-sentinel'))
        page.locator(core.SUBMIT_SELECTORS[0]).click = click
        result = await self.execute(page)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        click.assert_awaited_once()
        self.assertTrue(click.await_args.kwargs['trial'])
        self.context.cookies.assert_not_awaited()
