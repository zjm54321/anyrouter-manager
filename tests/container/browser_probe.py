"""Fixture launched ONLY by the production Rust worker; never an outer probe."""
import asyncio
import json
import os
from pathlib import Path
import sys


async def main():
    # Synthetic, bounded stdin only. No account, config, or environment secrets.
    assert json.loads(sys.stdin.buffer.readline(1024)) == {}
    sys.path.insert(0, "/app/tools/browser-helper")
    from browser_helper.core import browser_launcher

    browser = await browser_launcher()(headless=True)
    context = await browser.new_context()
    await context.route("**/*", lambda route: route.abort())
    page = await context.new_page()
    await page.goto("about:blank")
    assert await page.evaluate("1 + 1") == 2
    Path("ready").write_text(str(os.getpid()))
    while not Path("finish").exists():
        await asyncio.sleep(.01)
    # Only NORMAL receives finish. The intentional timeout is still cancelled
    # above, with the original supervisor deadlines/ownership and reap checks.
    sys.path.insert(0, "/app/tools/browser-helper/tests")
    from local_browser_smoke import notice_backdrop_cases, navigation_readiness_case

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
    await notice_backdrop_cases(page, report, render)
    await navigation_readiness_case(page, report)
    assert report == {"native_notice_backdrop_cases": 10, "native_navigation_readiness_cases": 1}
    await page.goto("about:blank")
    assert await page.evaluate("1 + 1") == 2
    await context.close()
    await browser.close()
    print('{"fixture":"done","notice_backdrop_cases":10,"notice_backdrop":"passed","navigation_readiness_cases":1}', flush=True)


if __name__ == "__main__":
    asyncio.run(main())
