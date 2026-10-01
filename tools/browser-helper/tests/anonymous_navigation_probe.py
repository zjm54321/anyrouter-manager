"""One bounded anonymous GET /login probe; never fills or submits credentials."""
import asyncio
from collections import Counter
import json
import os
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from browser_helper import core
from browser_helper.__main__ import reserve_protocol_fd, write_all


def error_code(exc):
    text = str(exc)
    # Whitelist codes only, never URL/query/body/stack/proxy details.
    codes = re.findall(r'(?:net::)?ERR_[A-Z_]+', text)
    if codes:
        return codes[0]
    lower = text.lower()
    if 'certificate' in lower or 'ssl' in lower:
        return 'tls_certificate_error'
    if 'timeout' in lower or isinstance(exc, TimeoutError):
        return 'timeout'
    if 'connection' in lower or 'socket' in lower:
        return 'connection_error'
    if 'libstdc++' in lower:
        return 'missing_native_library'
    return 'network_or_runtime_error'


async def probe(report):
    browser = context = None
    failures = Counter()
    request_failures = Counter()
    closing = False
    def failure(exc):
        if closing:
            return
        code = error_code(exc)
        failures[code] += 1
        report.setdefault('first_failure', {'type': type(exc).__name__, 'code': code})
    try:
        report['phase'] = 'launch'
        browser = await core.browser_launcher()(headless=True)
        report['launch_ok'] = True
        report['phase'] = 'new_context'
        context = await browser.new_context(service_workers='block')
        async def guard(route):
            request = route.request
            if not core.safe_url(request.url):
                report['cross_origin_aborted'] = report.get('cross_origin_aborted', 0) + 1
                await route.abort()
                return
            if request.method not in ('GET', 'HEAD') or request.post_data_buffer is not None:
                report['write_requests_aborted'] = report.get('write_requests_aborted', 0) + 1
                await route.abort()
                return
            try:
                fetched = await route.fetch(max_redirects=0, max_retries=0, timeout=25000)
                if 300 <= fetched.status < 400 or not core.safe_url(fetched.url):
                    report['redirect_aborted'] = report.get('redirect_aborted', 0) + 1
                    await route.abort()
                else:
                    await route.fulfill(response=fetched)
            except Exception as exc:
                failure(exc)
                await route.abort()
        await context.route('**/*', guard)
        page = await context.new_page()
        page.set_default_timeout(5000)
        def request_failed(request):
            if closing:
                return
            request_failures[error_code(RuntimeError(request.failure or 'request_failed'))] += 1
        page.on('requestfailed', request_failed)
        report['phase'] = 'goto'
        try:
            response = await page.goto(core.ORIGIN + '/login', wait_until='domcontentloaded', timeout=30000)
            report['main_http_status'] = response.status if response is not None else None
        except Exception as exc:
            failure(exc)
            report['goto_error'] = {'type': type(exc).__name__, 'code': error_code(exc)}
        report['phase'] = 'form_wait'
        deadline = asyncio.get_running_loop().time() + 30
        while True:
            report['same_origin_login'] = core.safe_url(page.url, '/login')
            report['username_visible'] = await core.first_visible(page, core.USERNAME_SELECTORS) is not None
            report['password_visible'] = await core.first_visible(page, core.PASSWORD_SELECTORS) is not None
            report['submit_visible'] = await core.first_visible(page, core.SUBMIT_SELECTORS) is not None
            report['challenge_visible'] = bool(await page.evaluate(core.CHALLENGE_JS))
            if all(report[k] for k in ('same_origin_login', 'username_visible', 'password_visible', 'submit_visible')) and not report['challenge_visible']:
                report['outcome'] = 'anonymous_login_form_available'
                break
            if failures:
                report['outcome'] = 'anonymous_navigation_network_failed'
                break
            if asyncio.get_running_loop().time() >= deadline:
                report['outcome'] = 'upstream_challenge' if report['challenge_visible'] or report.get('main_http_status') in (403, 429, 503) else 'login_form_unavailable'
                break
            await asyncio.sleep(0.5)
        report['cookie_names'] = sorted({c['name'] for c in await context.cookies()})
    finally:
        closing = True
        report['failure_categories'] = dict(failures)
        report['request_failed_categories'] = dict(request_failures)
        for key, resource in (('context', context), ('browser', browser)):
            if resource is not None:
                try:
                    await asyncio.wait_for(resource.close(), 5)
                    report[key + '_closed'] = True
                except Exception as exc:
                    report[key + '_close_error'] = error_code(exc)


def main():
    fd = reserve_protocol_fd()
    report = {'anonymous_only': True, 'main_http_status': None, 'proxy_presence': {
        key: bool(os.environ.get(key)) for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY')},
        'wrapper_proxy_none_resolves_empty': True}
    try:
        if os.readlink('/proc/self/ns/pid') == os.environ.get('HELPER_SMOKE_HOST_PIDNS'):
            raise RuntimeError('namespace_required')
        report['private_pid_namespace'] = True
        asyncio.run(asyncio.wait_for(probe(report), 80))
    except Exception as exc:
        report['probe_error'] = {'type': type(exc).__name__, 'code': error_code(exc)}
    write_all(fd, (json.dumps(report, separators=(',', ':')) + '\n').encode())
    os.close(fd)
    return 0 if report.get('browser_closed') else 1


if __name__ == '__main__':
    raise SystemExit(main())
