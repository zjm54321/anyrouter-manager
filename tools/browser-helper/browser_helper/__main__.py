"""One JSON request on stdin, one JSON result on stdout; never log secrets."""

import asyncio
import json
import logging
import os
import signal
import sys

from .core import Failure, MAX_INPUT_BYTES, failure_result, parse_input, run_login


class Discard:
    def write(self, text):
        return len(text)

    def flush(self):
        pass


def reserve_protocol_fd():
    """Process-entry only: fd 1/2 stay discarded until process exit.

    Restoring them would let delayed libc buffers/atexit hooks leak secrets.
    The non-inheritable duplicate is exclusively for the final protocol line.
    """
    protocol_fd = os.dup(1)
    os.set_inheritable(protocol_fd, False)
    logging.disable(sys.maxsize)
    with open(os.devnull, 'w') as null:
        for fd in (1, 2):
            os.dup2(null.fileno(), fd)
    sys.stdout = Discard()
    sys.stderr = Discard()
    return protocol_fd


def write_all(fd, payload):
    remaining = memoryview(payload)
    while remaining:
        try:
            written = os.write(fd, remaining)
        except InterruptedError:
            continue
        if written <= 0:
            raise OSError('protocol_write_failed')
        remaining = remaining[written:]


def read_request(stream):
    # Linux/Nix subprocess entry runs on the main thread. Bound incomplete pipes
    # too, so no newline/EOF cannot leave the helper hanging indefinitely.
    def expired(signum, frame):
        raise Failure('invalid_input')
    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, 5)
    try:
        raw = stream.readline(MAX_INPUT_BYTES + 1)
        return parse_input(raw)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def main(protocol_fd):
    result = failure_result('login_failed')
    try:
        credentials = read_request(sys.stdin.buffer)
        result = asyncio.run(run_login(credentials))
    except Failure as exc:
        result = failure_result(str(exc))
    except (KeyboardInterrupt, Exception):
        # Do not propagate wrapper/browser exception strings or tracebacks.
        pass
    write_all(protocol_fd, (json.dumps(result, ensure_ascii=True, allow_nan=False, separators=(',', ':')) + '\n').encode('ascii'))
    return 0 if result['ok'] else 1


if __name__ == '__main__':
    protocol_fd = reserve_protocol_fd()
    try:
        code = main(protocol_fd)
    finally:
        os.close(protocol_fd)
    raise SystemExit(code)
