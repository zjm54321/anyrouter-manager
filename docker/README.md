# Private internal-use container

This image targets **linux/amd64** only. It bundles the official, hash-verified,
unmodified CloakBrowser `146.0.7680.177.5` archive, Python 3.13 with locked
`cloakbrowser==0.5.11` / `playwright==1.58.0`, a release Rust backend, and the built
Vite frontend. There is no frontend dev server or runtime browser download.
Node 24 and the stable Bookworm Rust builder follow upstream tag updates;
application dependency resolution uses the existing lockfiles. The uv build tool
is pinned to the verified PyPI release `0.11.3`.

## License and publication boundary

Use this image only for private personal/internal purposes permitted by the
separate CloakBrowser binary terms; GPL applies to original application code,
not to the entire bundled browser. Consult `/app/THIRD_PARTY_NOTICES.md` and
`/opt/cloakbrowser/PROJECT-BINARY-LICENSE.md`. All archive notices are retained.
Public distribution or serving third-party customers may require a separate
CloakHQ OEM license. This workflow never builds in a public repository and never
publishes for pull requests. It publishes only from trusted `main` when `main` is
also the private repository's default branch, after successful runtime checks.

GHCR packages initially default to private, but existing package visibility can
be changed independently. Confirm the package is private before enabling the
workflow and after its first publication; do not change it to public. The workflow
also blocks publication to an existing non-private package and checks visibility
after publishing. API errors other than an absent package fail closed. Avoid
concurrent manual visibility changes during publication. No remote
or package is created by this implementation. CI uses only `GITHUB_TOKEN` with
`packages: write` in the trusted publication job, not account/login credentials.
Tags are `sha-<full commit SHA>` and `main`. No registry cache or build records
are uploaded by this workflow.

The trusted job builds/loads once, smokes the immutable loaded image ID, and
tags/pushes that same ID using Docker CLI. It never rebuilds after testing.
Registry login and all publication steps require the smoke's fail-closed
`publish_ready=true` output. PR jobs have read-only permissions and no registry login.
On supervisor capability denial CI explicitly reports **build-only evidence**, leaves the
image runner-local (no uploaded image artifact), and does not publish. A successful
build-only job is **not** proof of a functional runtime.

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

The config contract remains `container_mode`, `cookie_secure`, and
`browser_helper_executable`. No local Docker engine was available for this change:
**standard Docker operation is pending the first actual CI run**, not claimed as
tested or guaranteed. A local engine is required for the commands below; this
implementation does not install one.

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
single JSON data file is persistent. Config is mounted read-only and is never
part of an image layer. The image runs as UID/GID 1000 behind tini with Docker's
default seccomp policy and private PID namespace; retain both.

Visit `http://localhost:8080`. Exact allowed origins are also used to constrain
Host checks; `0.0.0.0` is a bind address, not a trusted client origin. Keep local
publication loopback-only. For remote HTTPS, use exact trusted HTTPS origins and
`cookie_secure = true`; configure that secure transport separately. No Compose,
Kubernetes, Ingress, or node security profiles are included.

`GET /health/live` and `/health/ready` expose only fixed safe health states and
require a permitted Host. The built-in probe uses `Host: localhost:8080` and
checks JSON readiness without printing a body. If you remove localhost from the
allowed health Hosts or change port, override the healthcheck accordingly.

## Validation

`python3 -B tests/container/test_contract.py` checks the static contract with no
Docker engine, network, or real credentials. The workflow additionally builds
the image, runs frontend tests/typecheck/build and Python mock tests, and executes
eight explicitly whitelisted pure Rust tests using `cargo test --locked ... --exact`.
The Rust binary test target is compiled/listed, but compilation is not execution.
The gate reports executed and deferred counts (currently 8 executed / 37 deferred
out of 45). Integration tests are not copied into or executed by the Docker build;
the full Rust regression suite is not implied by this gate. Namespace-dependent
tests and Rust parent-monitor fatal/permit tests remain separate native validation,
**not passed by these container probes**. No test executable is shipped in the image.
`docker/rust-tests.sh` fails if any whitelist name executes zero tests. CI then runs
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

The owner must run the private-repository workflow remotely for the first real
Docker result. Until it passes, do not publish or describe the image as verified
under default Docker. Local static checks: `python3 -B tests/container/test_contract.py`.
