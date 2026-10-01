"""Local health/web/auth checks; stdin carries synthetic CI root only."""
import http.client
import json
import sys


def request(path, *, method="GET", headers=None):
    connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=3)
    try:
        connection.request(method, path, headers={"Host": "localhost:8080", **(headers or {})})
        response = connection.getresponse()
        return response.status, dict(response.getheaders()), response.read(65537)
    finally:
        connection.close()


def probe(root):
    # Readiness is a separate capability gate, not part of the web/admin smoke.
    for path in ("/health/live",):
        status, headers, body = request(path)
        assert status == 200 and headers.get("content-type", "").startswith("application/json")
        assert isinstance(json.loads(body), dict)
    status, headers, body = request("/")
    assert status == 200 and b"<html" in body.lower()
    status, _, body = request("/api/account")
    assert status == 401 and json.loads(body)["error"]["code"] == "unauthorized"
    status, headers, _ = request("/api/admin/session", method="POST", headers={
        "Origin": "http://localhost:8080", "Authorization": "Bearer " + root,
    })
    assert status == 204 and "set-cookie" in headers
    cookie = headers["set-cookie"].split(";", 1)[0]
    status, _, body = request("/api/account", headers={"Cookie": cookie})
    assert status == 200 and isinstance(json.loads(body), dict)
    status, _, _ = request("/api/admin/session", method="DELETE", headers={
        "Origin": "http://localhost:8080", "Cookie": cookie,
    })
    assert status == 204


if __name__ == "__main__":
    try:
        probe(json.loads(sys.stdin.buffer.readline(1024))["root"])
    except Exception:
        print('{"http_probe":"failed"}')
        sys.exit(1)
    print('{"http_probe":"passed"}')
