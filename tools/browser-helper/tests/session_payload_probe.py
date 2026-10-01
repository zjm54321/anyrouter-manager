"""Parse-only stdin audit. Never imports/launches a browser or makes requests."""
import json
import sys
import time
from browser_helper import core


def accepted(raw):
    try:
        core.parse_input(raw)
        return True
    except core.Failure:
        return False


def audit(raw):
    data = json.loads(raw)
    cookies = data['cookies']
    normalized = dict(data, cookies=[dict(c, expires=-1 if c['expires'] is None else c['expires']) for c in cookies])
    live = dict(normalized, cookies=[c for c in normalized['cookies'] if c['expires'] == -1 or c['expires'] > time.time()])
    return {
        'parse_valid': accepted(raw),
        'cookie_count': len(cookies),
        'expiry_null_count': sum(c['expires'] is None for c in cookies),
        'expiry_session_number_count': sum(type(c['expires']) in (int, float) and c['expires'] == -1 for c in cookies),
        'expiry_past_count': sum(type(c['expires']) in (int, float) and c['expires'] >= 0 and c['expires'] <= time.time() for c in cookies),
        'timeout_integer_valid': type(data['timeout_ms']) is int and 1000 <= data['timeout_ms'] <= core.MAX_TIMEOUT_MS,
        'null_to_session_only_valid': accepted(json.dumps(normalized).encode()),
        'null_to_session_and_expired_removed_valid': accepted(json.dumps(live).encode()),
        'browser_launches': 0, 'upstream_requests': 0,
    }


if __name__ == '__main__':
    try:
        result = audit(sys.stdin.buffer.read(core.MAX_INPUT_BYTES + 1))
    except Exception:
        result = {'audit_failed': True, 'browser_launches': 0, 'upstream_requests': 0}
    print(json.dumps(result, sort_keys=True))
