"""Mock navigation timing only; network/subprocess tests keep their real clocks."""
import asyncio
import unittest
from unittest.mock import patch

from browser_helper import core
from test_helper import FakeBrowser, FakeContext, FakePage, response


class NavigationBudgetTests(unittest.IsolatedAsyncioTestCase):
    async def execute_navigation(self, page, *, launch_seconds=0, navigation_seconds=28,
                                 timeout_ms=40_000, never_ready=False):
        loop = asyncio.get_running_loop()
        original_sleep = asyncio.sleep
        # Exact epoch keeps integer-millisecond budgets independent of host uptime.
        now = 1000.0
        self.context = FakeContext(page)
        self.browser = FakeBrowser(self.context)
        self.state, self.navigation_calls, self.cancelled = {}, [], False
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
                    self.cancelled = True
                    raise
            return await original_goto(url, **kwargs)

        with patch.object(loop, 'time', lambda: now), patch.object(asyncio, 'sleep', sleep), \
                patch.object(page, 'goto', goto), patch.object(core, 'browser_launcher', return_value=launch):
            result = await core.run_login(core.LoginInput('synthetic-user', 'synthetic-password', timeout_ms), self.state)
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
                self.assertEqual(kwargs['wait_until'], 'domcontentloaded')
                self.assertIs(type(kwargs['timeout']), int)
                self.assertTrue(0 < kwargs['timeout'] <= 40_000 - launch_seconds * 1000)
                self.context.cookies.assert_awaited_once_with()
                page.locator(core.SUBMIT_SELECTORS[0]).click.assert_awaited_once()

    async def test_never_ready_navigation_cancelled_at_shared_deadline_without_mutation(self):
        page = FakePage(response())
        result = await self.execute_navigation(page, launch_seconds=2, timeout_ms=30_000, never_ready=True)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        url, kwargs = self.navigation_calls[0]
        self.assertEqual(url, core.ORIGIN + '/login')
        self.assertEqual(kwargs['wait_until'], 'domcontentloaded')
        self.assertIs(type(kwargs['timeout']), int)
        self.assertTrue(0 < kwargs['timeout'] <= 28_000)
        self.assertTrue(self.cancelled)
        self.assertEqual(self.state['phase'], 'navigation')
        self.assert_no_mutations(page)

    async def test_launch_cost_cannot_be_replaced_with_a_fresh_navigation_budget(self):
        page = FakePage(response())
        result = await self.execute_navigation(page, launch_seconds=15)
        self.assertEqual(result, {'ok': False, 'error': 'timeout'})
        url, kwargs = self.navigation_calls[0]
        self.assertEqual(url, core.ORIGIN + '/login')
        self.assertEqual(kwargs['wait_until'], 'domcontentloaded')
        self.assertIs(type(kwargs['timeout']), int)
        self.assertTrue(0 < kwargs['timeout'] <= 25_000)
        self.assertLess(kwargs['timeout'], 40_000)
        self.assert_no_mutations(page)

    async def test_action_timeout_retains_positive_integer_floor_and_existing_caps(self):
        loop = asyncio.get_running_loop()
        with patch.object(loop, 'time', return_value=100):
            for remaining in (0.0015, 0, -1):
                timeout = core.action_timeout({'deadline': 100 + remaining}, core.MAX_TIMEOUT_MS)
                self.assertIs(type(timeout), int)
                self.assertTrue(0 < timeout <= core.MAX_TIMEOUT_MS)
            self.assertEqual(core.action_timeout({'deadline': 221}, core.MAX_TIMEOUT_MS), core.MAX_TIMEOUT_MS)
            self.assertEqual(core.action_timeout({'deadline': 140}), 5000)
