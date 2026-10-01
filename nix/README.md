# Project-local CloakBrowser binary

`cloak-browser.nix` is a `{ pkgs }:` package expression adapted from the
[official flake at commit 7e626ee7a1b0e72ab2c9b98315c36302148df1ce](https://github.com/CloakHQ/CloakBrowser/blob/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/flake.nix).
It retains upstream's runtime libraries, fonts, autoPatchelfHook and wrappers.
It packages the **patched CloakBrowser binary**, not ordinary Chromium.

| System | Binary release | Official archive SHA-256 (SRI) |
| --- | --- | --- |
| x86_64-linux | 146.0.7680.177.5 | `sha256-ShK83pX6G7G+7ytBq15cJ8Nr544749DayMZNcFIWZw4=` |
| aarch64-linux | 146.0.7680.177.3 | `sha256-i3HOU7T9ExMnMxox+6ODXXGILRm/qr3njdD1OQvRb0U=` |

Archives come from
`https://cloakbrowser.dev/chromium-v<release>/cloakbrowser-<linux-x64|linux-arm64>.tar.gz`.
These are the fixed versions in the official Nix expression, **not** a claim
that they are the newest browser release (152), and **not** the Python helper's
`cloakbrowser==0.5.11` version. No latest-version lookup or fallback download is
part of this package.

The binary license is unfree and **nonredistributable**, as recorded in
`meta.license`. Do not publish its built output in a public binary cache.
The package deliberately does not enable `allowUnfree` globally.

## Root flake integration

The root flake now exposes `packages.<system>.cloakbrowser` and an opt-in
`devShells.<system>.browser`. `nix develop` retains the basic tools without a
browser dependency; `nix develop .#browser` adds the binary and sets the wrapper
path. Both package and browser shell use the narrow allowance below.

Within the existing per-system `let`, keep the normal `pkgs` import for other
dependencies and add a separate import allowing only this binary:

```nix
pkgs = import nixpkgs { inherit system; };
cloakPkgs = import nixpkgs {
  inherit system;
  config.allowUnfreePredicate = pkg:
    nixpkgs.lib.getName pkg == "cloakbrowser-chromium";
};
cloakBrowser = import ./nix/cloak-browser.nix { pkgs = cloakPkgs; };
```

Within `pkgs.mkShell`, preserve the current package list and append
`[ cloakBrowser ]`, then set:

```nix
CLOAKBROWSER_BINARY_PATH = "${cloakBrowser}/bin/cloakbrowser-chrome";
```

This points the helper at the Nix-patched wrapper, not the archive's raw ELF
binary. It requires neither global `nix-ld` configuration nor a system rebuild.
Use this environment variable for every helper launch; do not let the helper
download an unpatched browser on NixOS. Never enable a global unfree allowance
just to use this package.

## Standalone validation using the existing root lock

This expression reads the lock without changing it, imports its exact nixpkgs
revision and NAR hash, and limits the unfree allowance to this package:

```sh
nix build --impure --no-link --print-out-paths --expr '
  let
    lock = builtins.fromJSON (builtins.readFile /home/ming/anyrouter-manager/flake.lock);
    pinned = lock.nodes.nixpkgs.locked;
    nixpkgs = builtins.fetchTree {
      inherit (pinned) type owner repo rev narHash;
    };
    pkgs = import nixpkgs.outPath {
      system = "x86_64-linux";
      config.allowUnfreePredicate = pkg:
        (import (nixpkgs.outPath + "/lib")).getName pkg == "cloakbrowser-chromium";
    };
  in import /home/ming/anyrouter-manager/nix/cloak-browser.nix { inherit pkgs; }
'
```

Replace `nix build --impure --no-link --print-out-paths` with
`nix eval --impure --raw` and append `.drvPath` to the parenthesized final
package expression to evaluate only. Run the resulting
`<store-output>/bin/cloakbrowser-chrome --version` as the minimal local smoke.
A local `about:blank` Playwright smoke can use the existing helper virtualenv
and `CLOAKBROWSER_BINARY_PATH`; it must not visit AnyRouter, sign in, or execute
remote check-in scripts. Local smoke is not proof of WAF bypass.

NixOS MCP was not available to this implementation session (resource and
resource-template listings were empty); no `nix search` substitute was used.

### Validation record (2026-09-30)

- Existing root lock: nixpkgs `7a0f122f5090cf4c2ade2a13a0e229d4e19ba71f`,
  NAR hash `sha256-ZoxIApko70jCdbH3l20HWXOBaT2HZd87orzd2yJ9dVE=`.
- Derivation evaluation passed for both supported systems.
- x86_64 build was attempted with `--no-link`; the initial PTY execution
  failed with `fatal runtime error: assertion failed: output.write(&bytes).is_ok(), aborting`.
  Retrying without a PTY fetched native dependencies, but timed out after
  600 seconds while fetching the pinned official archive. Its fetch log
  repeatedly reported `curl: (56) OpenSSL SSL_read: OpenSSL/3.6.4: error:0A000126:SSL routines::unexpected eof while reading, errno 0`.
- At that earlier stage the browser output was not realized. The later
  checksum-verified resume and offline build below resolved that download
  blocker; no archive/hash/version substitution was made.
- aarch64 was evaluated only, not built or run on the x86_64 WSL2 host.
- No AnyRouter page, login, remote check-in script, or WAF test was run.
- After optional root integration, `nix flake check --no-build --no-update-lock-file`
  passed on x86_64 (aarch64 outputs omitted by that check). The default shell
  version check passed and confirmed `CLOAKBROWSER_BINARY_PATH` was unset:
  rustc 1.98.1, Cargo 1.98.0, rustfmt 1.9.0, Clippy 0.1.98, Node 24.20.0,
  npm 11.19.0, Python 3.14.7, uv 0.12.17, pkg-config 0.29.2.
- A bounded recovery diagnosed the original official URL redirecting to the
  same GitHub release asset. HEAD advertised `accept-ranges: bytes`; a 1024-byte
  Range probe returned HTTP 206, `content-range: bytes 0-1023/216890134`, with
  ETag `"0x8DEB6E6E582E458"`. Resume is technically supported.
- The one allowed recovery download used curl `--continue-at - --max-time 300`
  into `/tmp/opencode/cloakbrowser-linux-x64.tar.gz`, without changing package
  inputs. It ended with `curl: (28) Operation timed out after 300017 milliseconds with 51625966 out of 216890134 bytes received`.
  The incomplete file was not hash-accepted or imported into the Nix store;
  the subsequent build and local smoke steps therefore did not execute.
   Default-shell success is not browser success.
- A follow-up explicitly authorized a single bounded resume, reusing the
  existing 51,625,966-byte partial, exact URL, ETag and package hash. It uses
  `curl --continue-at - --max-time 1500` with `If-Range`, in a notified PTY with
  an 1800-second outer timeout. It must return HTTP 206 with the expected range,
  complete all 216,890,134 bytes and match `nix hash file --type sha256 --sri`
  before any store import. This follow-up **completed successfully** within
  its bound: curl exit 0, final HTTP 206,
  `content-range: bytes 51625966-216890133/216890134`, unchanged ETag
  `"0x8DEB6E6E582E458"`, final file size 216,890,134 bytes. `nix hash file`
  returned exactly `sha256-ShK83pX6G7G+7ytBq15cJ8Nr544749DayMZNcFIWZw4=`.
  The verified full archive remains at
  `/tmp/opencode/cloakbrowser-linux-x64.tar.gz`.
- The cache import used the normal Nix command
  `nix store add --mode flat --hash-algo sha256 --name cloakbrowser-linux-x64.tar.gz`,
  after checksum verification. Its result equaled the fetchurl derivation's
  actual fixed-output path (read using `nix derivation show`):
  `/nix/store/p7drswiia9j3m978r8mqkq44ay4k68ik-cloakbrowser-linux-x64.tar.gz`;
  no store paths were guessed or written manually. The subsequent build used
  `nix build .#cloakbrowser --offline --no-update-lock-file --no-link --print-out-paths -L`.
  **Build passed**, including native fixup:
  `auto-patchelf: 0 dependencies could not be satisfied`.
  Build log: `/tmp/opencode/cloakbrowser-offline-build.log`.
- Follow-up derivation evaluations still passed for x86_64 and aarch64, and
  the browser shell environment points at
  `/nix/store/sv231g34qjil8r23fc43i1mnlmyvgzhn-cloakbrowser-chromium-146.0.7680.177.5/bin/cloakbrowser-chrome`.
  `--version` passed with exit 0 and reported `Chromium 146.0.7680.177`;
  upstream's patched archive release suffix `.5` is not printed by that CLI.
- Local smoke ran with proxy environment variables removed and
  `unshare --user --map-root-user --net`, preventing external network access.
  `--headless --dump-dom data:text/html,...` returned exit 0 but **empty stdout**,
  both with and without `--disable-dev-shm-usage`; its title assertion failed.
  This CLI limitation was not accepted as a successful DOM smoke.
- A dependency-free CDP pipe probe of the same wrapper **passed**: browser
  `Browser.getVersion` reported `Chrome/146.0.7680.177`, then `Runtime.evaluate`
  on local `about:blank` returned
  `{"url":"about:blank","title":"cloak-local-js-ok"}` after setting
  `document.title`. This used only stdlib pipes/JSON and browser CLI, not the
  Python CloakBrowser wrapper or the helper virtualenv. Probe script:
  `/tmp/opencode/cloakbrowser-cdp-smoke.py`.
- At this browser-build stage Python wrapper startup had not been tested;
  the native import/driver-only follow-up below does not launch a browser.
  Real login/WAF behavior remains untested regardless of local CLI smoke.
  The headless probes used `--no-sandbox` inside an isolated user/network
  namespace; they do not validate production sandbox configuration.

### Process-group boundary probe (local only)

An isolated `about:blank` probe launched the wrapper with
`start_new_session=True`. Actual observed IDs in its second run:

- Probe parent PID/PGID/SID: `65049`; browser launch PID/PGID/SID: `65050`.
- Chromium zygotes `65057`, `65058`, GPU `65078`, network utility `65085`,
  storage utility `65086`, renderers `65116`, `65117` all kept PGID/SID `65050`.
- **Crashpad escaped the browser process group/session**: handler PID `65052`,
  PGID/SID `65051`, PPID `48689` after reparenting. It was linked to this browser
  by the children's exact `--crashpad-handler-pid=65052` argument.

Therefore a timeout implementation that only kills the launch PGID does not
cover every browser-associated process; this probe is not a claim that the
backend/helper timeout path has passed. That owner must validate its actual
helper launch/cleanup path, including detached Crashpad. The probe tracked
only its own descendants/unique temporary profile and linked handler, checked
PID start times before signaling escaped processes, and never killed other
browser jobs. Its cleanup required no final forced live-PID kill. Raw record:
`/tmp/opencode/cloakbrowser-pgid.json`; script:
`/tmp/opencode/cloakbrowser-pgid-probe.py`.

### Python native wheels / driver-only follow-up (2026-09-30)

The already-installed helper dependencies initially failed actual import with
`ImportError: libstdc++.so.6: cannot open shared object file: No such file or directory`.
`ldd` on `greenlet/_greenlet.cpython-314-x86_64-linux-gnu.so` confirmed only
`libstdc++.so.6 => not found`; other listed libraries were resolved. This is
distinct from passing helper mocks.

Both default and browser dev shells now use the minimal runtime library set
`pkgs.lib.makeLibraryPath [ pkgs.stdenv.cc.cc.lib ]`, prepended by `shellHook`
to inherited `LD_LIBRARY_PATH` without deleting existing entries. This keeps
the loader path project-local, adds no desktop runtime set, and does not
change the browser wrapper's own library prefix or global Nix configuration.

The installed Playwright `_impl/_driver.py` `compute_driver_executable()`
explicitly supports `PLAYWRIGHT_NODEJS_PATH` on Linux (line 33). Its bundled
Node ELF requests `/lib64/ld-linux-x86-64.so.2`; both shells therefore set
`PLAYWRIGHT_NODEJS_PATH = "${pkgs.nodejs_24}/bin/node"`. No wheel file was
patched. This selects the driver Node runtime, **not a replacement browser**.

Performed actual validation through:

```sh
nix develop .#browser --offline --no-update-lock-file --command \
  uv run --frozen --project tools/browser-helper python -B -c '<import + async driver start/stop probe>'
```

- Passed imports: `cloakbrowser`, `playwright.async_api`, `greenlet`.
- Installed metadata: Python `3.14.7`, CloakBrowser wrapper `0.5.11`,
  Playwright `1.58.0`, greenlet `3.5.6`.
- Callable wrapper APIs verified: `launch`, `launch_async`,
  `launch_context_async`, `launch_persistent_context_async`; none were called.
- `compute_driver_executable()` selected the configured Nix Node:
  `/nix/store/v6wsd9nyglmwh32yn7w8z11dq9cksg4m-nodejs-24.20.0/bin/node`,
  version `v24.20.0`. The JS CLI remains the installed Playwright wheel's
  `driver/package/cli.js`.
- `async_playwright().start()` and `.stop()` both passed with no browser launch,
  no Crashpad, no website navigation or credentials.
- Default-shell actual greenlet import and `ldd` also passed, resolving
  `libstdc++.so.6` from the pinned GCC runtime with no `not found` entries;
  default shell still had no `CLOAKBROWSER_BINARY_PATH`.
- No dependencies were re-synced, no wheel files patched, no lock/input changes,
  and no backend/helper/frontend files modified. The previously reported 37
  helper mock tests were not rerun by this environment task.
- NixOS MCP availability was checked again: resource/template lists were
  empty and no NixOS info tool was exposed; no `nix search` substitute was used.

Native import and driver startup blockers are resolved on this x86_64 host.
Actual Python-wrapper browser launch remains deferred to the namespace/safety
gate; backend timeout cleanup (including detached Crashpad), real login and
WAF behavior are still not validated by this check. After the namespace gate
owner reported its tested command, `pkgs.util-linux` was explicitly added to
both shells' shared `basePackages` to provide `unshare`. This task does not
change the production wrapper or namespace invocation and does not retest
browser launch.

The final browser-shell import/driver start/stop check passed again after
adding util-linux, using `nix develop .#browser --no-update-lock-file --command
uv run --frozen --project tools/browser-helper python -B -c '<probe>'`.
`unshare --version` returned `unshare from util-linux 2.42.3`; the resolved
explicit shell executable was
`/nix/store/mqvbf0flamqaq9c3ihb496aahag897n0-util-linux-2.42.3-bin/bin/unshare`.
The final default-shell offline check also passed greenlet import/`ldd`,
confirmed that same explicit `unshare`, and kept the browser env absent.

An initial `--offline` attempt after adding util-linux had no cached output
and attempted a source build, which failed in upstream tinycc's fixed-output
source: expected `sha256-MRuqq3TKcfIahtUWdhAcYhqDiGPkAjS8UTMsDE+/jGU=`,
got `sha256-Mhn1p2HVKc2swmQFdic1i5Up8b9w7MjceOnri9SBDDA=`. The ordinary
locked Nix invocation then realized util-linux successfully; no source hashes,
flake inputs or locks were changed to bypass this failure. The successful
driver check is the final runtime result, not a claim that a cacheless source
build of util-linux was fixed.

### 真实账号实测流程记录（2026-09-30）

后续由父代理在隔离受控环境中完成本次实测（详见 [docs/verification.md](../docs/verification.md)）：
- 基于 `.#browser` shell 与锁定 Nix/Python 运行环境，通过无头浏览器完成本次 AnyRouter 防护页加载与登录。
- 成功读取原始额度并提取现有 Key（1 个），激活后通过本地 `/v1/models` 代理成功获取 16 个模型列表 JSON（HTTP 200）。
- 测试未创建新 Key，未调用付费模型，测试结束临时配置和会话文件已删除。
