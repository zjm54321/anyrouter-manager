# Container build and public publication

This image targets **linux/amd64** only. It bundles the official, hash-verified,
unmodified CloakBrowser `146.0.7680.177.5` archive, Python 3.13 with locked
`cloakbrowser==0.5.11` / `playwright==1.58.0`, a release Rust backend, and the built
Vite frontend. There is no frontend dev server or runtime browser download.
Node 24 and the stable Bookworm Rust builder follow upstream tag updates;
application dependency resolution uses the existing lockfiles. The uv build tool
is pinned to the verified PyPI release `0.11.3`.
The backend uses Rust edition 2024 (Rust 1.85 or newer) and Cargo lockfile v4;
the stable Rust builder must also satisfy the locked dependencies' MSRV. Only
the actual locked CI compile/test gate proves that compatibility; local static
checks do not claim the floating builder has been executed.

## License and publication boundary

GPL-3.0-or-later applies to original application code, not to the entire bundled
browser. The Python CloakBrowser wrapper is MIT-licensed; Chromium and bundled
components retain their own licenses. The compiled CloakBrowser has separate
proprietary terms: the CloakBrowser Binary License v1.0 (February 2026) does
**not** grant default public-redistribution rights. Its personal/internal-use
permission is not permission to distribute a bundled public image or serve
third-party customers; those uses require separate permission from CloakHQ.

For **this delivery only**, the deployment owner has confirmed explicit
redistribution permission covering the pinned `146.0.7680.177.5` browser in a
public container. The publisher relies on that confirmation and is responsible
for remaining within the obtained grant. This does not amend the upstream
license or grant other publishers redistribution rights; no private grant text
is committed. Consult [the project notices](../THIRD_PARTY_NOTICES.md), also at
`/app/THIRD_PARTY_NOTICES.md`, and the full pinned license at
`/opt/cloakbrowser/PROJECT-BINARY-LICENSE.md`. Original archive notices and the
upstream wrapper license are retained in `/opt/cloakbrowser/`.

The `Public container` workflow validates pull requests without registry login,
publication, or uploaded image artifacts. Publication is restricted to a
**public** repository's trusted `main`, which must also be its default branch,
after successful mandatory production runtime checks. CI uses only
`GITHUB_TOKEN`, with `packages: write` only in that trusted publication job;
checkout does not persist credentials. No account credentials or PAT are needed.
Tags are only `sha-<full commit SHA>` and `main`. No registry cache or build records
are uploaded by this workflow.

The expected GHCR package must **already exist and be public** before publication.
The workflow checks that exact repository-derived package before login and again
after pushing. A private/internal/missing package, HTTP 404, other API errors, or
an unrecognized response blocks publication. It does not create a first private
package or change visibility automatically. Avoid concurrent manual visibility
changes during publication. These checks enforce visibility, not license rights.

### Automatic publication after push

Pushing a commit to the public repository's default branch **`main`** automatically
starts this workflow: static validation → build/load once → mandatory offline
production runtime smoke → existing-public visibility guard → GHCR login →
tag/push the exact tested image ID → verify existing-public visibility.
`workflow_dispatch` is only an optional retry, with no required inputs. No per-push
environment variables or PAT secret are needed. The image name is derived from
`github.repository`, lowercased: for a hypothetical `Owner/Repo`, the tags are
`ghcr.io/owner/repo:main` and `ghcr.io/owner/repo:sha-<full-commit-SHA>`.
Pull requests can build/test but cannot log in or publish; non-main pushes are
not subscribed. Keep `main` as the default branch or deliberately update both
the push trigger and trusted-publication branch checks together.

Operator prerequisites: use a **public** GitHub repository, enable Actions, and
permit this workflow's `GITHUB_TOKEN` to write packages under repository/org
policy. Confirm the expected public GHCR package already exists and grants this
repository's Actions access in package settings (inherited repository permissions
may already supply access). First-package creation is outside this workflow:
GHCR initially defaults new packages to private, so an absent package deliberately
blocks the job. No token belongs in config, source, Docker build arguments, or
image layers. A successful static check is not evidence of a Docker build or a
functional container; the actual CI build and runtime gates must still pass for
the revision being published.

The trusted job builds/loads once, smokes the immutable loaded image ID, and
tags/pushes that same ID using Docker CLI. It never rebuilds after testing.
Registry login and all publication steps require the smoke's fail-closed
`publish_ready=true` output. PR jobs have read-only permissions and no registry login.
On supervisor capability denial CI explicitly reports **build-only evidence**, leaves the
image runner-local (no uploaded image artifact), and does not publish. The trusted
publication job fails instead of presenting a green runtime result; a PR may retain
explicit build-only evidence, which is **not** proof of a functional runtime.

## Runtime prerequisites — fail closed

With explicit `container_mode = true`, the backend launches an independent,
single-thread Rust subreaper supervisor before Tokio initialization. It starts
the helper, observes control-pipe cancellation and parent death, and reaps its
own adopted descendants (including double-fork/setsid Crashpad children) using
pidfds. Only cleanup ACK plus worker reap releases the backend browser permit.
This is **lifecycle management, not a namespace-equivalent sandbox**.

Keep Docker's normal **private container PID namespace and the image's tini
entrypoint**. Never use `--pid=host`, a shared PID namespace, or bypass tini with
an entrypoint that leaves the container alive after the backend exits. A worker
crash, missing ACK, or cleanup deadline failure fatally exits the service; tini
then exits and container PID namespace teardown is the final cleanup boundary.
Do not run the fatal-failure path as a host service with `container_mode=true`.

The standard container path no longer calls `unshare` and does not require
extra capabilities, privileged mode, unconfined policies, or host configuration
changes. Kernel/policy support for subreaper, pidfd, signalfd, and own-child `/proc`
inspection must still actually pass the capability/runtime probes. Capability
denial fails closed; readiness checks the supervisor without starting a browser.
The old user/PID `unshare` prerequisite remains **only for non-container mode**;
there is no automatic fallback between the modes.

The config contract includes `container_mode`, `cookie_secure`,
`browser_helper_executable`, and the default-off `allow_insecure_lan_http` opt-in.
**Static/mock validation does not prove standard Docker operation**; this
revision still requires successful actual CI runtime checks. A local engine is
required for the commands below; this implementation does not install one.

## Private local run

1. Copy `docker/config.example.toml` **outside the repository** to a private
   config file. Replace the placeholder with a random high-entropy root key of
   at least 32 ASCII bytes using a private editor/generator. Never use build
   args, environment variables, CLI root-key arguments, or a committed file.
2. Make that file mode 0600 and readable by container UID/GID 1000. Prepare a
   private data directory owned by 1000:1000, mode 0700. The backend creates
   `/data/private/account.json` with its private directory/file permissions.
3. Build and run (substitute your own absolute mount paths):

```sh
docker build --platform linux/amd64 -t anyrouter-manager:local .
docker run --name anyrouter-manager --detach \
  --platform linux/amd64 --read-only --cap-drop ALL \
  --security-opt no-new-privileges=true \
  --publish 127.0.0.1:8080:8080 \
  --mount type=bind,src=/absolute/private/config.toml,dst=/config/config.toml,readonly \
  --mount type=bind,src=/absolute/private/data,dst=/data \
  --tmpfs /tmp:rw,nosuid,nodev,size=512m,mode=1777 \
  --shm-size 256m --memory 2g --pids-limit 512 \
  anyrouter-manager:local
```

These are resource-budget examples, not a promise every browser fits 2 GiB.
All browser profiles, helper cache, and HOME live in ephemeral `/tmp`; only the
plain JSON state file is persistent: it contains sensitive session cookies and
account/key state, not a saved login password. Its mode/ownership and private
backups matter; it is not encrypted storage. Config is mounted read-only and is never
part of an image layer. The image runs as UID/GID 1000 behind tini with Docker's
default seccomp policy and private PID namespace; retain both.

Visit `http://localhost:8080`. Exact allowed origins are also used to constrain
Host checks; `0.0.0.0` is a bind address, not a trusted client origin. The example
keeps port exposure loopback-only. Optional private-LAN HTTP requires explicit
`allow_insecure_lan_http = true`, `container_mode = true`, `cookie_secure = false`,
and exact allowed origins using literal IPv4 addresses in `192.168.0.0/16`.
This default-off opt-in accepts cleartext transport only within the owner's
trusted private-LAN security boundary; it does not encrypt root credentials,
session cookies, or traffic. Do not expose it to untrusted networks. No Compose,
Kubernetes, Ingress, or node security profiles are included.

`GET /health/live` and `/health/ready` expose only fixed safe health states and
require a permitted Host. The built-in probe uses `Host: localhost:8080` and
checks JSON readiness without printing a body. If you remove localhost from the
allowed health Hosts or change port, override the healthcheck accordingly.

## Validation

`python3 -B tests/container/test_contract.py` checks the static contract with no
Docker engine, network, or real credentials. `python3 -B tests/container/test_workflow.py`
also evaluates automatic-push/public/default-main conditions and runs the publication
shell against a fake Docker CLI, including changed-ID and push-failure cases.
The workflow additionally builds
the image, runs frontend tests/typecheck/build and Python mock tests, and executes
26 explicitly whitelisted pure/in-process Rust tests using `cargo test --locked ... --exact`.
The Rust binary test target is compiled/listed, but compilation is not execution.
The gate reports executed and deferred counts dynamically (26 whitelisted tests,
including the four LAN-HTTP config and in-process session tests;
all other listed binary tests are deferred). Integration tests are not copied into or executed by the Docker build;
the full Rust regression suite is not implied by this gate. Namespace-dependent
tests and Rust parent-monitor fatal/permit tests remain separate native validation,
**not passed by these container probes**. No test executable is shipped in the image.
`docker/rust-tests.sh` fails if any whitelist name executes zero tests. CI then runs
the runtime smoke below. When backend tests are renamed or moved, update the reviewed
whitelist and its static source checks together; do not silently accept zero matches.
Adding unrelated tests does not require hardcoding a new suite count. CI runs
`tests/container/ci_smoke.py`. That script uses an isolated random **synthetic**
root config and no external network. Web/static/liveness/admin checks are separate
from the mandatory production supervisor/browser/readiness gate. Capability denial
returns exit 2 with structured `blocked` and `publish_ready=false`, not a passing
readiness result. The observer calls the exact loaded release binary's internal
worker protocol, not bare Python or a replacement supervisor. It exercises normal
exit, timeout, cancellation, SIGTERM, and parent SIGKILL of synthetic double-fork/
setsid descendants. The parent-death observer retains the control writer to test
PDEATHSIG independently from control EOF. A distinct ACK channel must report
cleanup, and all pinned descendant pidfds must show **reaped**, not merely exited,
at ACK receipt and **before** container deletion. Normal workers must exit 0;
orphan workers must be reaped by tini. This tests the production worker, not the
Rust parent monitor's fatal-exit/permit behavior.

The actual CloakBrowser wrapper also runs **inside that same Rust supervisor**,
opens only `about:blank`, aborts browser HTTP routes, and tests normal exit and
timeout. Observed browser/Crashpad pidfds must be reaped before ACK. Docker smoke
uses `--network none`; no fixture needs credentials or an account. Scripts are
copied into disposable `/tmp`, not into the final image.
It never attempts an AnyRouter login, inference,
check-in, or key creation. It prints fixed summaries only and removes its own
container and temporary config. Removal is teardown, not descendant-cleanup
evidence. A failed supervisor/browser/cleanup/readiness probe blocks publication.
No Docker build, runtime, Rust build-stage execution, or Actions execution is
claimed from local static/mock validation without an engine.

The owner must obtain the public-repository workflow's actual build/runtime result
for this revision. Until the mandatory runtime gates pass, publication must remain
blocked; do not describe the image as verified under default Docker based on local
static checks. Local static checks: `python3 -B tests/container/test_contract.py`.
