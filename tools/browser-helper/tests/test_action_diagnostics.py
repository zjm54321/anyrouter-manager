import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch, AsyncMock, Mock
from types import SimpleNamespace

from browser_helper import core

ROOT = Path(__file__).resolve().parents[1]


class ActionDiagnosticsTests(unittest.IsolatedAsyncioTestCase):
    async def test_inflight_action_omits_elapsed_and_clears_previous_measurement(self):
        state = {'phase': 'submit'}
        with patch.object(core, 'time', SimpleNamespace(monotonic=Mock(side_effect=[10, 10.25, 11, 11.5]))):
            with core.diagnostic_action(state, 'submit_trial', 321):
                pass
            self.assertEqual(core.diagnostics(state)['action_elapsed_ms'], 250)
            with core.diagnostic_action(state, 'page_recheck'):
                await asyncio.sleep(0)
                self.assertNotIn('action_elapsed_ms', state)
                result = core.diagnostics(state)
                self.assertEqual(result['action'], 'page_recheck')
                self.assertNotIn('action_timeout_ms', result)
                self.assertNotIn('action_elapsed_ms', result)
            self.assertEqual(core.diagnostics(state)['action_elapsed_ms'], 500)

    async def test_completed_action_preserves_measured_zero_and_elapsed_bound(self):
        for finished, elapsed in ((10, 0), (131, 120000)):
            with self.subTest(elapsed=elapsed):
                state = {'phase': 'submit'}
                with patch.object(core, 'time', SimpleNamespace(monotonic=Mock(side_effect=[10, finished]))):
                    with core.diagnostic_action(state, 'submit_click', 4321):
                        pass
                result = core.diagnostics(state)
                self.assertEqual(result['action_elapsed_ms'], elapsed)
                self.assertEqual(result['action_timeout_ms'], 4321)

    async def test_notice_operations_keep_actual_budgets(self):
        for failing in ('notice_close', 'notice_wait_hidden'):
            close = SimpleNamespace(is_visible=AsyncMock(return_value=True), is_enabled=AsyncMock(return_value=True), click=AsyncMock())
            dialog = SimpleNamespace(is_visible=AsyncMock(return_value=True), evaluate=AsyncMock(return_value=True), locator=Mock(return_value=close), wait_for=AsyncMock())
            dialogs = SimpleNamespace(count=AsyncMock(return_value=1), nth=Mock(return_value=dialog))
            page = SimpleNamespace(url=core.ORIGIN + '/login', locator=Mock(return_value=dialogs))
            state = {'phase': 'submit', 'deadline': 999}
            target = close.click if failing == 'notice_close' else dialog.wait_for
            target.side_effect = TimeoutError('secret-sentinel')
            with patch.object(core, 'require_safe_page', AsyncMock(return_value=False)), patch.object(core, 'action_timeout', side_effect=[321, 123]) as budget:
                with self.assertRaises(TimeoutError):
                    await core.dismiss_notice_once(page, state)
            self.assertEqual(core.diagnostics(state)['action'], failing)
            self.assertEqual(core.diagnostics(state)['action_timeout_ms'], 321 if failing == 'notice_close' else 123)
            self.assertEqual(budget.call_count, 1 if failing == 'notice_close' else 2)
            close.click.assert_awaited_once_with(timeout=321)
            if failing == 'notice_wait_hidden':
                dialog.wait_for.assert_awaited_once_with(state='hidden', timeout=123)
            self.assertEqual(state['deadline'], 999)

    async def test_each_action_failure_has_bounded_metadata(self):
        for action in core.DIAGNOSTIC_ACTIONS:
            state = {'phase': 'submit', 'deadline': 999}
            timeout = None if action == 'page_recheck' else 4321
            ticks = iter([10, 10.25])
            with patch.object(core, 'time', SimpleNamespace(monotonic=lambda: next(ticks))):
                with self.assertRaises(TimeoutError):
                    with core.diagnostic_action(state, action, timeout):
                        await asyncio.sleep(0)
                        raise TimeoutError('secret-password-cookie-sentinel')
            result = core.diagnostics(state)
            self.assertEqual(result['action'], action)
            self.assertEqual(result['action_elapsed_ms'], 250)
            self.assertEqual(result.get('action_timeout_ms'), timeout)
            self.assertEqual(state['deadline'], 999)
            self.assertNotIn('sentinel', json.dumps(result))

    async def test_cancellation_and_later_phase(self):
        state = {'phase': 'submit'}
        with self.assertRaises(asyncio.CancelledError):
            with core.diagnostic_action(state, 'submit_click', 12):
                raise asyncio.CancelledError()
        self.assertGreaterEqual(core.diagnostics(state)['action_elapsed_ms'], 0)
        state['phase'] = 'profile_wait'
        self.assertNotIn('action', core.diagnostics(state))
        with core.diagnostic_action(state, 'notice_close', 12):
            pass
        self.assertNotIn('action_timeout_ms', core.diagnostics(state))

    async def test_allowlist_and_numeric_bounds(self):
        for value in [-1, 120001, True, 'password']:
            result = core.diagnostics({'phase': 'submit', 'action': 'submit_trial',
                                       'action_elapsed_ms': value, 'action_timeout_ms': value})
            self.assertNotIn('action_elapsed_ms', result)
            self.assertNotIn('action_timeout_ms', result)
        self.assertNotIn('action', core.diagnostics({'phase': 'submit', 'action': 'secret-url'}))


class ActionWatchdogTests(unittest.TestCase):
    def test_real_module_watchdog_omits_unknown_inflight_elapsed(self):
        # Run the actual entry/watchdog with synthetic work only. signal.pause()
        # blocks the event loop until the unchanged process deadline exits it.
        script = '''
import runpy, signal
from types import SimpleNamespace
from unittest.mock import Mock, patch
from browser_helper import core

async def blocked(credentials, state):
    state.update(phase='submit', page='login', login_requested=False, self_requested=False)
    with patch.object(core, 'time', SimpleNamespace(monotonic=Mock(side_effect=[10, 10.25]))):
        with core.diagnostic_action(state, 'submit_trial', 321):
            pass
    assert state['action_elapsed_ms'] == 250
    with core.diagnostic_action(state, 'page_recheck'):
        signal.pause()
    raise AssertionError('watchdog did not terminate synthetic work')

core.run_login = blocked
runpy.run_module('browser_helper', run_name='__main__')
'''
        # subprocess.run kills and waits for the child if this outer bound fires.
        process = subprocess.run(
            [sys.executable, '-B', '-c', script],
            input=b'{"username":"synthetic-user","password":"synthetic-password","timeout_ms":1000}\n',
            capture_output=True, cwd=ROOT,
            env=dict(os.environ, BROWSER_HELPER_DIAGNOSTICS='1', CLOAKBROWSER_BINARY_PATH=''),
            timeout=6,
        )
        self.assertEqual(process.returncode, 1)
        self.assertEqual(process.stderr, b'')
        self.assertEqual(process.stdout.count(b'\n'), 1)
        self.assertNotIn(b'synthetic-', process.stdout)
        result = json.loads(process.stdout)
        self.assertFalse(result['ok'])
        self.assertEqual(result['error'], 'timeout')
        details = result['diagnostics']
        self.assertEqual(details['phase'], 'submit')
        self.assertEqual(details['page'], 'login')
        self.assertEqual(details['exception'], 'timeout')
        self.assertEqual(details['action'], 'page_recheck')
        self.assertFalse(details['login_requested'])
        self.assertFalse(details['self_requested'])
        self.assertNotIn('action_timeout_ms', details)
        self.assertNotIn('action_elapsed_ms', details)


if __name__ == '__main__':
    unittest.main()
