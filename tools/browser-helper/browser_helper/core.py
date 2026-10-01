"""Login flow with bounded lifetime and positively verified profile responses."""

from __future__ import annotations

import asyncio
import importlib
import json
import math
import os
import re
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

ORIGIN = "https://anyrouter.top"
MAX_INPUT_BYTES = 16_384
DEFAULT_TIMEOUT_MS = 60_000
MAX_TIMEOUT_MS = 120_000
FORM_WAIT_SECONDS = 15
CLEANUP_SECONDS = 2
USERNAME_SELECTORS = ('#username', 'input[name="username"]', 'input[name="email"]', 'input[type="email"]')
PASSWORD_SELECTORS = ('#password', 'input[name="password"]', 'input[type="password"]')
SUBMIT_SELECTORS = ('form.semi-form button[type="submit"]', 'button[type="submit"]')
ENTRY_SELECTORS = (
    '.semi-card button:has(.semi-icon-mail)',
    '.semi-card button:has([aria-label="mail"])',
)
ENTRY_NAMES = re.compile(r"邮箱或用户名|使用.*邮箱|Email or Username|Sign in with Email", re.I)
COOKIE_NAME = re.compile(r"^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$")
COOKIE_VALUE = re.compile(r"^[\x21\x23-\x2b\x2d-\x3a\x3c-\x5b\x5d-\x7e]*$")

CHALLENGE_JS = r"""() => {
    const visible = el => {
        const style = getComputedStyle(el), rect = el.getBoundingClientRect();
        return style.display !== 'none' && style.visibility !== 'hidden'
            && Number(style.opacity) !== 0 && rect.width > 0 && rect.height > 0;
    };
    const text = document.body?.innerText || '';
    if (/请进行验证|为了更好的访问体验|访问受限|Access denied|verify you are human|checking your browser|just a moment/i.test(text)) return true;
    return [...document.querySelectorAll('iframe[src*="captcha"], iframe[src*="verify"], iframe[src*="slide"], iframe[src*="challenges.cloudflare.com"], .nc-container, #nocaptcha, .cf-turnstile, .g-recaptcha')].some(visible);
}"""

# Return only a validated ID and document readiness, never storage contents.
USER_STATE_JS = r"""() => {
    let id = null;
    try {
        const user = JSON.parse(localStorage.getItem('user'));
        if (user && Number.isSafeInteger(user.id) && user.id > 0) id = user.id;
    } catch {}
    return {id, ready: document.readyState === 'interactive' || document.readyState === 'complete'};
}"""

DIAGNOSTIC_ENUMS = {
    'phase': ('launch', 'navigation', 'form_wait', 'fill', 'submit', 'profile_wait', 'cookie_read', 'done'),
    'page': ('login', 'console', 'other'),
    'self_id_type': ('absent', 'integer', 'string', 'other'),
    'failure_request': ('none', 'navigation', 'login', 'self', 'resource'),
    'exception': ('none', 'timeout', 'navigation_transient', 'network', 'unexpected'),
}
DIAGNOSTIC_BOOLS = ('login_requested', 'login_json', 'self_requested', 'self_json',
                    'self_id_valid', 'self_user_header_present', 'user_state_ready', 'pending_login')


def diagnostics(state):
    """Construct the fixed contract; never copy raw dictionaries into output."""
    result = {'version': 1}
    for key, values in DIAGNOSTIC_ENUMS.items():
        value = state.get(key)
        result[key] = value if type(value) is str and value in values else ('other' if key == 'page' else values[0])
    for key in DIAGNOSTIC_BOOLS:
        result[key] = state.get(key) is True
    for key in ('login_status', 'self_status'):
        value = state.get(key)
        result[key] = value if type(value) is int and 100 <= value <= 599 else None
    for key in ('login_success', 'self_success'):
        value = state.get(key)
        result[key] = value if type(value) is bool else None
    return result


def failure_result(code, state=None):
    result = {'ok': False, 'error': code}
    if os.environ.get('BROWSER_HELPER_DIAGNOSTICS') == '1':
        result['diagnostics'] = diagnostics(state or {})
    return result


def exception_kind(exc):
    if isinstance(exc, TimeoutError):
        return 'timeout'
    # Keep Playwright lazy too: invalid input / missing binary needs no runtime.
    try:
        api = importlib.import_module('playwright.async_api')
        if isinstance(exc, api.TimeoutError):
            return 'timeout'
        if isinstance(exc, api.Error) and ('Execution context was destroyed' in str(exc)
                                          or 'Cannot find context with specified id' in str(exc)):
            return 'navigation_transient'
        if isinstance(exc, api.Error) and ('net::ERR_' in str(exc) or 'Navigation failed' in str(exc)):
            return 'network'
    except ImportError:
        pass
    return 'unexpected'


def action_timeout(state, cap=5000):
    return max(1, min(cap, int((state['deadline'] - asyncio.get_running_loop().time()) * 1000)))


async def safe_evaluate(page, script, state):
    for attempt in range(3):
        if not safe_url(page.url):
            raise Failure('challenge_or_block')
        try:
            return await page.evaluate(script)
        except Exception as exc:
            kind = exception_kind(exc)
            state['exception'] = kind
            if kind != 'navigation_transient':
                raise
            if attempt == 2 or asyncio.get_running_loop().time() >= state['deadline']:
                raise Failure('user_self_unverified' if state['submitted'] else 'login_form_unavailable') from None
            await asyncio.sleep(0.05)


def request_kind(request):
    if safe_url(request.url, '/api/user/login') and getattr(request, 'method', '') == 'POST':
        return 'login'
    if safe_url(request.url, '/api/user/self') and getattr(request, 'method', '') == 'GET':
        return 'self'
    if getattr(request, 'is_navigation_request', lambda: False)():
        return 'navigation'
    return 'resource'


class Failure(Exception):
    """Only fixed, non-sensitive error codes may leave the helper."""


@dataclass(frozen=True, repr=False)
class LoginInput:
    username: str
    password: str
    timeout_ms: int


def parse_input(raw: bytes) -> LoginInput:
    if not raw or len(raw) > MAX_INPUT_BYTES or b'\n' in raw.rstrip(b'\r\n'):
        raise Failure("invalid_input")
    try:
        def unique_object(pairs):
            result = {}
            for key, value in pairs:
                if key in result:
                    raise ValueError
                result[key] = value
            return result
        data = json.loads(raw.decode('utf-8'), object_pairs_hook=unique_object)
        if not isinstance(data, dict) or set(data) - {'username', 'password', 'timeout_ms'}:
            raise ValueError
        username, password = data['username'], data['password']
        timeout = data.get('timeout_ms', DEFAULT_TIMEOUT_MS)
        if not isinstance(username, str) or not username.strip() or len(username.encode('utf-8')) > 320:
            raise ValueError
        if not isinstance(password, str) or not password or len(password.encode('utf-8')) > 4096:
            raise ValueError
        if any(ord(c) < 32 or ord(c) == 127 for c in username + password):
            raise ValueError
        if type(timeout) is not int or not 1000 <= timeout <= MAX_TIMEOUT_MS:
            raise ValueError
    except (ValueError, KeyError, TypeError, UnicodeError, RecursionError):
        raise Failure("invalid_input") from None
    return LoginInput(username, password, timeout)


def safe_url(url: str, path: str | None = None) -> bool:
    try:
        if not isinstance(url, str) or any(ord(c) <= 32 or ord(c) == 127 for c in url):
            return False
        parsed = urlsplit(url)
        return (
            parsed.scheme == 'https' and parsed.hostname == 'anyrouter.top'
            and parsed.port in (None, 443) and parsed.username is None
            and parsed.password is None and not parsed.fragment
            and (path is None or parsed.path == path)
        )
    except (ValueError, TypeError):
        return False


def profile_id(payload: object) -> str | None:
    if not isinstance(payload, dict) or payload.get('success') is not True:
        return None
    data = payload.get('data')
    value = data['id'] if isinstance(data, dict) and 'id' in data else payload.get('id')
    # JSON integer, not bool/float/string; bounded to Rust signed 64-bit ID.
    return str(value) if type(value) is int and 0 < value <= 2**63 - 1 else None


async def verified_response(response) -> str | None:
    if response.status != 200 or not safe_url(response.url, '/api/user/self'):
        return None
    request = response.request
    while request is not None:
        if not safe_url(request.url):
            return None
        request = request.redirected_from
    try:
        return profile_id(await response.json())
    except Exception:
        return None


def filtered_cookies(cookies: list[dict], source_url: str) -> list[dict]:
    if not safe_url(source_url, '/console'):
        return []
    result = []
    for cookie in cookies:
        if not isinstance(cookie, dict):
            continue
        name, value = cookie.get('name'), cookie.get('value')
        path, expires = cookie.get('path'), cookie.get('expires')
        if cookie.get('domain') not in ('anyrouter.top', '.anyrouter.top'):
            continue
        if not isinstance(name, str) or not COOKIE_NAME.fullmatch(name):
            continue
        if not isinstance(value, str) or not COOKIE_VALUE.fullmatch(value):
            continue
        if not isinstance(path, str) or not path.startswith('/') or any(ord(c) < 32 or ord(c) == 127 for c in path):
            continue
        if type(expires) not in (int, float) or not math.isfinite(expires) or expires < -1:
            continue
        if type(cookie.get('secure')) is not bool or type(cookie.get('httpOnly')) is not bool:
            continue
        result.append(dict(name=name, value=value, domain=cookie['domain'], path=path,
                           secure=cookie['secure'], http_only=cookie['httpOnly'], expires=expires))
    return result


def browser_launcher():
    # Check before importing the wrapper: no download, auto-install or fallback.
    binary = os.environ.get('CLOAKBROWSER_BINARY_PATH', '')
    if not binary or not Path(binary).is_absolute() or not Path(binary).is_file() or not os.access(binary, os.X_OK):
        raise Failure('browser_unavailable')
    try:
        return importlib.import_module('cloakbrowser').launch_async
    except Exception:
        raise Failure('browser_unavailable') from None


async def first_visible(page, selectors):
    for selector in selectors:
        locator = page.locator(selector).first
        if await locator.is_visible() and await locator.is_enabled():
            return locator
    return None


async def require_safe_page(page, state):
    if not safe_url(page.url):
        raise Failure('challenge_or_block')
    state['page'] = 'login' if safe_url(page.url, '/login') else ('console' if safe_url(page.url, '/console') else 'other')
    return bool(await safe_evaluate(page, CHALLENGE_JS, state))


async def find_form(page, state):
    state['phase'] = 'form_wait'
    deadline = min(state['deadline'], asyncio.get_running_loop().time() + FORM_WAIT_SECONDS)
    opened = False
    while True:
        if state['network_failed']:
            raise Failure('login_failed')
        if not safe_url(page.url, '/login'):
            raise Failure('login_form_unavailable')
        state['challenge'] = await require_safe_page(page, state)
        if state['challenge']:
            # Passive wait only: never click/fill a challenge. It may resolve by
            # itself; the overall deadline, not a short DOM timer, is the cap.
            await asyncio.sleep(0.1)
            continue
        username = await first_visible(page, USERNAME_SELECTORS)
        password = await first_visible(page, PASSWORD_SELECTORS)
        submit = await first_visible(page, SUBMIT_SELECTORS)
        if username and password and submit and await username.is_editable() and await password.is_editable():
            state['challenge'] = False
            state['http_block'] = False
            return username, password, submit
        if asyncio.get_running_loop().time() >= deadline:
            if state['http_block']:
                state['challenge'] = True
                await asyncio.sleep(0.1)
                continue
            raise Failure('login_form_unavailable')
        if not opened:
            entry = await first_visible(page, ENTRY_SELECTORS)
            if entry is None:
                candidate = page.get_by_role('button', name=ENTRY_NAMES).first
                if await candidate.is_visible() and await candidate.is_enabled():
                    entry = candidate
            if entry is not None:
                await entry.click(timeout=action_timeout(state))
                opened = True
        await asyncio.sleep(min(0.2, max(0.001, deadline - asyncio.get_running_loop().time())))


async def login_work(credentials, launch, resources, state):
    try:
        resources['browser'] = await launch(headless=True)
    except Exception as exc:
        if exception_kind(exc) == 'timeout':
            raise
        raise Failure('browser_unavailable') from None
    browser = resources['browser']
    resources['context'] = await browser.new_context(service_workers='block')
    context = resources['context']

    async def guard_route(route):
        request = route.request
        # continue_ lets Chromium follow 307/308 and resend a POST body without
        # routing the redirect. Fetch EVERY request without following redirects,
        # then fulfill only the non-redirect result. No third-party allowlist.
        kind = request_kind(request)
        if kind == 'navigation' and getattr(request, 'frame', None) != page.main_frame:
            kind = 'resource'
        critical = kind != 'resource'
        if not safe_url(request.url):
            if critical:
                state.update(network_failed=True, failure_request=kind, exception='network')
            await route.abort()
            return
        login = kind == 'login' and state['submitted']
        if login:
            state.update(login_requested=True, pending_login=True)
        elif kind == 'self' and state['submitted']:
            state['self_requested'] = True
            state['self_user_header_present'] = 'new-api-user' in getattr(request, 'headers', {})
        try:
            remaining_ms = max(1, int((state['deadline'] - asyncio.get_running_loop().time()) * 1000))
            fetched = await route.fetch(max_redirects=0, max_retries=0, timeout=remaining_ms)
            if kind in ('login', 'self') and state['submitted']:
                state[kind + '_status'] = fetched.status
            if 300 <= fetched.status < 400 or not safe_url(fetched.url):
                if critical:
                    state.update(network_failed=True, failure_request=kind, exception='network')
                await route.abort()
            else:
                await route.fulfill(response=fetched)
        except Exception as exc:
            if critical:
                state.update(network_failed=True, failure_request=kind,
                             exception='timeout' if exception_kind(exc) == 'timeout' else 'network')
            await route.abort()
        finally:
            if login:
                state['pending_login'] = False

    await context.route('**/*', guard_route)
    page = await context.new_page()
    page.set_default_timeout(5000)
    state['phase'] = 'navigation'
    response = await page.goto(ORIGIN + '/login', wait_until='domcontentloaded', timeout=20_000)
    state['http_block'] = response is not None and response.status in (403, 429, 503)
    captured = None
    login_error = None
    tasks = set()

    async def capture(response):
        nonlocal captured, login_error
        kind = request_kind(response.request)
        path = '/api/user/login' if kind == 'login' else '/api/user/self'
        if kind not in ('login', 'self') or not safe_url(response.url, path):
            return
        request = response.request
        while request is not None:
            if not safe_url(request.url) or request.redirected_from is not None:
                return
            request = request.redirected_from
        prefix = 'login' if kind == 'login' else 'self'
        state[prefix + '_requested'] = True
        state[prefix + '_status'] = response.status
        if kind == 'self':
            state['self_user_header_present'] = 'new-api-user' in getattr(response.request, 'headers', {})
        try:
            payload = await response.json()
        except Exception as exc:
            if exception_kind(exc) == 'timeout':
                state['exception'] = 'timeout'
            if kind == 'login':
                login_error = 'challenge_or_block' if response.status in (403, 429, 503) else 'login_failed'
            return
        state[prefix + '_json'] = True
        success = payload.get('success') if isinstance(payload, dict) else None
        state[prefix + '_success'] = success if type(success) is bool else None
        if kind == 'login':
            if response.status != 200 or success is not True:
                login_error = 'challenge_or_block' if response.status in (403, 429, 503) else 'login_failed'
        else:
            data = payload.get('data') if isinstance(payload, dict) else None
            has_id = isinstance(payload, dict) and ('id' in payload or isinstance(data, dict) and 'id' in data)
            value = data['id'] if isinstance(data, dict) and 'id' in data else (payload.get('id') if isinstance(payload, dict) else None)
            state['self_id_type'] = 'absent' if not has_id else ('integer' if type(value) is int else ('string' if type(value) is str else 'other'))
            user = profile_id(payload) if response.status == 200 else None
            state['self_id_valid'] = user is not None
            if user is not None:
                captured = user

    def on_response(response):
        if not state['submitted']:
            return
        if request_kind(response.request) not in ('login', 'self'):
            return
        task = asyncio.create_task(capture(response))
        tasks.add(task)
        task.add_done_callback(tasks.discard)

    page.on('response', on_response)
    try:
        await find_form(page, state)
        # Revalidate immediately before each credential mutation and submission.
        for index, value in ((0, credentials.username), (1, credentials.password)):
            form = await find_form(page, state)
            if not safe_url(page.url, '/login'):
                raise Failure('login_form_unavailable')
            state['phase'] = 'fill'
            await form[index].fill(value, timeout=action_timeout(state))
        _, _, submit = await find_form(page, state)
        if not safe_url(page.url, '/login'):
            raise Failure('login_form_unavailable')
        state['submitted'] = True
        state['phase'] = 'submit'
        # Never retry submit: a timed out click may already have sent credentials.
        await submit.click(timeout=action_timeout(state))
        state['phase'] = 'profile_wait'
        navigated = False
        previous_ready = None
        ready_samples = 0
        while True:
            if login_error:
                state['challenge'] = await require_safe_page(page, state)
                raise Failure('challenge_or_block' if state['challenge'] else login_error)
            if state['network_failed']:
                raise Failure('timeout' if state['exception'] == 'timeout' else ('user_self_unverified' if state['failure_request'] == 'self' else 'login_failed'))
            state['challenge'] = await require_safe_page(page, state)
            on_console = safe_url(page.url, '/console')
            if not state['challenge'] and state['login_success'] is True:
                user_state = await safe_evaluate(page, USER_STATE_JS, state)
                value = user_state.get('id') if isinstance(user_state, dict) else None
                ready = (isinstance(user_state, dict) and user_state.get('ready') is True
                         and type(value) is int and 0 < value <= 2**53 - 1)
                state['user_state_ready'] = ready
                snapshot = (page.url, value) if ready else None
                ready_samples = ready_samples + 1 if snapshot is not None and snapshot == previous_ready else (1 if ready else 0)
                previous_ready = snapshot
                if captured is not None and ready and str(value) != captured:
                    raise Failure('user_self_unverified')
                if captured is not None and on_console and ready_samples >= 2 and not state['pending_login']:
                    state['http_block'] = False
                    break
                # Observe SPA navigation first. A completed login alone is NOT
                # ready: require a stable safe page AND validated user storage.
                if not navigated and ready_samples >= 2 and not state['pending_login'] and (not on_console or not state.get('self_requested', False)):
                    if not on_console and not safe_url(page.url, '/login'):
                        raise Failure('user_self_unverified')
                    navigated = True
                    response = await page.goto(ORIGIN + '/console', wait_until='domcontentloaded', timeout=20_000)
                    state['http_block'] = response is not None and response.status in (403, 429, 503)
                    continue
            await asyncio.sleep(0.05)
        state['phase'] = 'cookie_read'
        cookies = filtered_cookies(await context.cookies(), page.url)
        if not cookies:
            raise Failure('user_self_unverified')
        state['phase'] = 'done'
        return {'ok': True, 'cookies': cookies, 'api_user': captured}
    finally:
        page.remove_listener('response', on_response)
        pending = list(tasks)
        for task in pending:
            task.cancel()
        if pending:
            await asyncio.gather(*pending, return_exceptions=True)


async def close_resources(resources):
    # Browser close is attempted even if context close fails or hangs.
    for key in ('context', 'browser'):
        resource = resources.get(key)
        if resource is not None:
            try:
                await asyncio.wait_for(resource.close(), timeout=CLEANUP_SECONDS / 2)
            except Exception:
                pass


async def run_login(credentials: LoginInput) -> dict:
    resources = {}
    state = dict(deadline=asyncio.get_running_loop().time() + credentials.timeout_ms / 1000,
                 submitted=False, pending_login=False, network_failed=False, challenge=False,
                 http_block=False, phase='launch', login_success=None)
    try:
        launch = browser_launcher()
        return await asyncio.wait_for(login_work(credentials, launch, resources, state), credentials.timeout_ms / 1000)
    except Failure as exc:
        return failure_result(str(exc), state)
    except Exception as exc:
        kind = exception_kind(exc)
        if kind != 'timeout':
            state['exception'] = kind
            if kind == 'network':
                state['failure_request'] = 'navigation' if state['phase'] == 'navigation' else ('self' if state.get('self_requested') else 'login')
                return failure_result('user_self_unverified' if state['failure_request'] == 'self' else 'login_failed', state)
            return failure_result('login_failed', state)
        error = 'challenge_or_block' if state['challenge'] or state['http_block'] else (
            'login_form_unavailable' if state['phase'] == 'form_wait' else (
                'user_self_unverified' if state['phase'] == 'profile_wait' and state['login_success'] is True and not state['pending_login'] else 'timeout'))
        state['exception'] = 'timeout'
        return failure_result(error, state)
    finally:
        await close_resources(resources)
