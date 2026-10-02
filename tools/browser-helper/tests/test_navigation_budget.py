"""Mock navigation timing only; network/subprocess tests keep their real clocks."""
import asyncio
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

from browser_helper import core
from test_helper import FakeBrowser, FakeContext, FakePage, Locator, response


class NavigationBudgetTests(unittest.IsolatedAsyncioTestCase):
    async def execute_navigation(self, page, *, launch_seconds=0, navigation_seconds=28,
                                 timeout_ms=40_000, never_ready=False, final_url=None):
        loop = asyncio.get_running_loop()
        original_time, original_sleep = loop.time, asyncio.sleep
        # Exact epoch keeps integer-millisecond budgets independent of host uptime.
        now = start = 1000.0
        self.context = FakeContext(page)
        self.browser = FakeBrowser(self.context)
        self.state, self.navigation_calls, self.cancelled = {}, [], []
        original_goto = page.goto

        async def sleep(delay, result=None):
            nonlocal now
            now += max(0, delay)
            return await original_sleep(0, result)

        async def launch(**kwargs):
            await asyncio.sleep(launch_seconds)
            return self.browser

        async def goto(url, **kwargs):
            self.navigation_calls.append((url, kwargs))
            if url == core.ORIGIN + '/login':
                try:
                    if never_ready:
                        # The real run_login wait_for must cancel this Future;
                        # no mocked timeout result or bypass of its timer queue.
                        await asyncio.sleep(self.state['deadline'] - loop.time())
                        await asyncio.Future()
                    await asyncio.sleep(min(navigation_seconds, kwargs['timeout'] / 1000))
                    if navigation_seconds * 1000 > kwargs['timeout']:
                        raise TimeoutError('synthetic navigation timeout')
                except asyncio.CancelledError:
                    self.cancelled.append(loop.time() - start)
                    raise
            result = await original_goto(url, **kwargs)
            if final_url is not None:
                page.url = final_url
            return result

        with patch.object(loop, 'time', lambda: now), patch.object(asyncio, 'sleep', sleep), \
                patch.object(page, 'goto', goto), patch.object(core, 'browser_launcher', return_value=launch):
            result = await core.run_login(core.LoginInput('synthetic-user', 'synthetic-password', timeout_ms), self.state)
            self.elapsed = loop.time() - start
        self.assertEqual(loop.time, original_time)
        self.assertIs(asyncio.sleep, original_sleep)
        self.context.close.assert_awaited_once()
        self.browser.close.assert_awaited_once()
        self.assertEqual(page.listeners, {})
        return result

    def assert_no_mutations(self, page):
        for locator in page.inputs.values():
            locator.fill.assert_not_awaited()
            locator.click.assert_not_awaited()
        self.context.cookies.assert_not_awaited()
        self.assertFalse(self.state['submitted'])

    async def test_navigation_over_twenty_seconds_uses_remaining_deadline(self):
        for launch_seconds in (0, 7.125):
            with self.subTest(launch_seconds=launch_seconds):
                page = FakePage(response())
                result = await self.execute_navigation(page, launch_seconds=launch_seconds)
                self.assertTrue(result['ok'])
                self.assertEqual(result['api_user'], '42')
                url, kwargs = self.navigation_calls[0]
                self.assertEqual(url, core.ORIGIN + '/login')
                self.assertEqual(kwargs, {'wait_until': 'domcontentloaded',
                                          'timeout': 40_000 - int(launch_seconds * 1000)})
                self.assertLess(self.elapsed, 40)
                self.context.cookies.assert_awaited_once_with()
                page.locator(core.SUBMIT_SELECTORS[0]).click.assert_awaited_once()

    async def test_never_ready_navigation_cancelled_at_shared_deadline_without_mutation(self):
        page = FakePage(response())
        result = await self.execute_navigation(page, launch_seconds=2, timeout_ms=30_000, never_ready=True)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        self.assertEqual(self.navigation_calls[0][1], {'wait_until': 'domcontentloaded', 'timeout': 28_000})
        self.assertEqual(self.cancelled, [30])
        self.assertEqual(self.elapsed, 30)
        self.assertEqual(self.state['phase'], 'navigation')
        self.assert_no_mutations(page)

    async def test_launch_cost_cannot_be_replaced_with_a_fresh_navigation_budget(self):
        page = FakePage(response())
        # This ambient origin rounded the old remaining budget down to 24999ms.
        with patch.object(asyncio.get_running_loop(), 'time', return_value=1000.1):
            result = await self.execute_navigation(page, launch_seconds=15)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        self.assertEqual(self.navigation_calls[0][1], {'wait_until': 'domcontentloaded', 'timeout': 25_000})
        self.assertEqual(self.elapsed, 40)
        self.assert_no_mutations(page)

    async def test_delayed_unsafe_path_challenge_and_unknown_document_never_fill(self):
        for mode in ('off_origin', 'wrong_path', 'challenge', 'unknown_html'):
            with self.subTest(mode=mode):
                page = FakePage(response(), challenge=mode == 'challenge', form=mode != 'unknown_html')
                final_url = {'off_origin': 'https://example.invalid/login',
                             'wrong_path': core.ORIGIN + '/not-login'}.get(mode)
                result = await self.execute_navigation(page, final_url=final_url)
                self.assertEqual(result, {'ok': False, 'error':
                                         'challenge_or_block' if mode == 'challenge' else 'login_form_unavailable'})
                self.assert_no_mutations(page)

    async def test_delayed_normal_notice_then_one_submit_verified_profile_and_cookies(self):
        page = FakePage(response())
        original_locator = page.locator
        close, dialog = Locator(), Locator()
        dialog.locator = lambda selector: close
        dialog.wait_for = AsyncMock()
        group = SimpleNamespace(count=AsyncMock(return_value=1), nth=lambda index: dialog)
        page.locator = lambda selector: group if selector.startswith(':is(') else original_locator(selector)
        submit = original_locator(core.SUBMIT_SELECTORS[0])
        original_click = submit.click

        async def click(**kwargs):
            close.click.assert_awaited_once()
            dialog.wait_for.assert_awaited_once()
            return await original_click(**kwargs)

        submit.click = AsyncMock(side_effect=click)
        result = await self.execute_navigation(page)
        self.assertTrue(result['ok'])
        self.assertEqual(result['api_user'], '42')
        self.assertEqual(len(result['cookies']), 1)
        self.assertFalse(await core.dismiss_notice_once(page, self.state))
        close.click.assert_awaited_once()
        self.assertEqual(dialog.wait_for.await_args.kwargs['state'], 'hidden')
        self.assertEqual(submit.click.await_count, 2)
        self.assertEqual([call.kwargs.get('trial', False) for call in submit.click.await_args_list], [True, False])
        for locator in (original_locator(core.USERNAME_SELECTORS[0]), original_locator(core.PASSWORD_SELECTORS[0])):
            locator.fill.assert_awaited_once()
            self.assertLessEqual(locator.fill.await_args.kwargs['timeout'], 5000)
        self.assertTrue(all(0 < call.kwargs['timeout'] <= 5000 for call in submit.click.await_args_list))
        self.context.cookies.assert_awaited_once_with()

    async def test_action_timeout_retains_positive_integer_floor_and_existing_caps(self):
        loop = asyncio.get_running_loop()
        with patch.object(loop, 'time', return_value=100):
            for remaining, expected in ((28.125, 28_125), (0.0015, 1), (0, 1), (-1, 1), (121, 120_000)):
                state = {'deadline': 100 + remaining}
                self.assertEqual(core.action_timeout(state, core.MAX_TIMEOUT_MS), expected)
                self.assertIs(type(core.action_timeout(state, core.MAX_TIMEOUT_MS)), int)
            self.assertEqual(core.action_timeout({'deadline': 140}), 5000)
