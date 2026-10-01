"""Fail-closed visibility guard; never print tokens, API bodies, or exceptions."""
import json
import os
import sys
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen


def private_or_new(require_existing=False):
    repository = os.environ["REPOSITORY"].lower().split("/")
    if len(repository) != 2 or not all(repository):
        return False
    owner, package = repository
    owner_type = os.environ["OWNER_TYPE"]
    if owner_type not in ("User", "Organization"):
        return False
    scope = "orgs" if owner_type == "Organization" else "users"
    request = Request(
        f"https://api.github.com/{scope}/{quote(owner, safe='')}/packages/container/{quote(package, safe='')}",
        headers={"Authorization": "Bearer " + os.environ["GH_TOKEN"],
                 "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"},
    )
    try:
        with urlopen(request, timeout=15) as response:
            result = json.loads(response.read(65537))
        return result.get("visibility") == "private"
    except HTTPError as error:
        # An absent package is created private by GHCR. Every other failure blocks.
        return error.code == 404 and not require_existing


if __name__ == "__main__":
    try:
        passed = private_or_new(require_existing="--require-existing" in sys.argv[1:])
    except Exception:
        passed = False
    print(json.dumps({"ghcr_visibility": "passed" if passed else "blocked"}))
    sys.exit(0 if passed else 1)
