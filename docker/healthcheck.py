"""Fixed local readiness probe; never print response bodies or exceptions."""
import http.client
import sys


def ready():
    connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=3)
    try:
        connection.request("GET", "/health/ready", headers={"Host": "localhost:8080"})
        response = connection.getresponse()
        return response.status == 200 and response.getheader("Content-Type", "").startswith("application/json")
    except Exception:
        return False
    finally:
        connection.close()


if __name__ == "__main__":
    sys.exit(0 if ready() else 1)
