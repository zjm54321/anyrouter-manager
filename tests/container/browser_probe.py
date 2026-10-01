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
    await context.close()
    await browser.close()
    print('{"fixture":"done"}', flush=True)


if __name__ == "__main__":
    asyncio.run(main())
