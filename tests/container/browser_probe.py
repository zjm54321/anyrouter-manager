"""Fixture launched ONLY by the production Rust worker; never an outer probe."""
import asyncio
import json
import os
from pathlib import Path
import sys


PHASES = ('launch', 'context_setup', 'ready_wait', 'fixture_import', 'notice',
          'navigation', 'proactive_self', 'final_check', 'close')
CATEGORIES = ('timeout', 'assertion', 'import', 'os_error', 'unexpected')
ASSERTIONS = ('helper_result', 'returned_cookie', 'server_observation')
HELPER_ERRORS = ('invalid_input', 'browser_unavailable', 'login_failed', 'timeout',
                 'challenge_or_block', 'login_form_unavailable', 'user_self_unverified', 'session_expired')


def browser_failure(value):
    """Strict wire boundary shared by both observers; never forward extra fields."""
    required = {'fixture', 'phase', 'category'}
    if (type(value) is not dict or not required <= value.keys()
            or value.keys() - required - {'assertion_kind', 'helper_error'}
            or any(type(v) is not str for v in value.values())
            or value['fixture'] != 'failed' or value['phase'] not in PHASES
            or value['category'] not in CATEGORIES):
        return None
    if 'assertion_kind' in value and (value['phase'] != 'proactive_self'
            or value['category'] != 'assertion' or value['assertion_kind'] not in ASSERTIONS):
        return None
    if 'helper_error' in value and (value.get('assertion_kind') != 'helper_result'
                                   or value['helper_error'] not in HELPER_ERRORS):
        return None
    return dict(value)


def exception_report(phase, exc, proactive_type=None):
    # Playwright's TimeoutError is distinct from the builtin. No message matching.
    api = sys.modules.get('playwright.async_api')
    timeout_type = getattr(api, 'TimeoutError', TimeoutError)
    category = ('timeout' if isinstance(exc, (TimeoutError, timeout_type)) else
                'assertion' if isinstance(exc, AssertionError) else
                'import' if isinstance(exc, ImportError) else
                'os_error' if isinstance(exc, OSError) else 'unexpected')
    report = {'fixture': 'failed', 'phase': phase, 'category': category}
    if phase == 'proactive_self' and proactive_type is not None and isinstance(exc, proactive_type):
        if type(exc.kind) is str and exc.kind in ASSERTIONS:
            report['assertion_kind'] = exc.kind
            if exc.kind == 'helper_result' and type(exc.helper_error) is str and exc.helper_error in HELPER_ERRORS:
                report['helper_error'] = exc.helper_error
    return browser_failure(report)


async def run(state):
    # Synthetic, bounded stdin only. No account, config, or environment secrets.
    assert json.loads(sys.stdin.buffer.readline(1024)) == {}
    sys.path.insert(0, "/app/tools/browser-helper")
    from browser_helper.core import browser_launcher

    browser = await browser_launcher()(headless=True)
    state['phase'] = 'context_setup'
    context = await browser.new_context(service_workers='block')
    await context.route("**/*", lambda route: route.abort())
    page = await context.new_page()
    await page.goto("about:blank")
    assert await page.evaluate("1 + 1") == 2
    Path("ready").write_text(str(os.getpid()))
    state['phase'] = 'ready_wait'
    while not Path("finish").exists():
        await asyncio.sleep(.01)
    # Only NORMAL receives finish. The intentional timeout is still cancelled
    # above, with the original supervisor deadlines/ownership and reap checks.
    sys.path.insert(0, "/app/tools/browser-helper/tests")
    state['phase'] = 'fixture_import'
    from local_browser_smoke import notice_backdrop_cases, navigation_readiness_case, proactive_self_case, ProactiveSelfFailure
    state['proactive_type'] = ProactiveSelfFailure

    async def render(html):
        async def serve(route):
            await route.fulfill(status=200, content_type="text/html; charset=utf-8", body=html)
        # Page route overrides the context abort only for this synthetic document.
        # No upstream request, credential, extra browser or full probe invocation.
        url = "https://anyrouter.top/login"
        await page.route(url, serve)
        try:
            await page.goto(url, wait_until="domcontentloaded")
        finally:
            await page.unroute(url, serve)

    report = {}
    state['phase'] = 'notice'
    await notice_backdrop_cases(page, report, render)
    state['phase'] = 'navigation'
    await navigation_readiness_case(page, report)
    state['phase'] = 'proactive_self'
    await proactive_self_case(browser, context, page, report)
    state['phase'] = 'final_check'
    assert report == {"native_notice_backdrop_cases": 10, "native_navigation_readiness_cases": 1, "native_proactive_self_cases": 1}
    await page.goto("about:blank")
    assert await page.evaluate("1 + 1") == 2
    state['phase'] = 'close'
    await context.close()
    await browser.close()
    print('{"fixture":"done","notice_backdrop_cases":10,"notice_backdrop":"passed","navigation_readiness_cases":1,"proactive_self_cases":1}', flush=True)


async def main():
    state = {'phase': 'launch'}
    try:
        await run(state)
    except Exception as exc:
        print(json.dumps(exception_report(state['phase'], exc, state.get('proactive_type'))), flush=True)
        return 1  # Rust still sees failure and owns descendant cleanup.
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
