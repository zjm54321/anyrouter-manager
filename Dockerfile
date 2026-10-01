# syntax=docker/dockerfile:1
# Internal-use image only: CloakBrowser binaries have separate proprietary terms.
# Build only linux/amd64; application dependencies remain locked, base tags track updates.
FROM node:24-bookworm-slim AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm test && npm run typecheck && npm run build

FROM rust:bookworm AS backend
WORKDIR /build/backend
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/src/ ./src/
COPY docker/rust-tests.sh /build/rust-tests.sh
# Execute 13 reviewed pure tests; production supervisor probes run on the loaded image.
RUN /bin/sh /build/rust-tests.sh && cargo build --locked --release

FROM python:3.13-slim-bookworm AS helper
ENV UV_PYTHON_DOWNLOADS=never UV_LINK_MODE=copy PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
RUN pip install --no-cache-dir uv==0.11.3
WORKDIR /app/tools/browser-helper
COPY tools/browser-helper/pyproject.toml tools/browser-helper/uv.lock ./
RUN uv sync --frozen --no-dev --python /usr/local/bin/python3.13
COPY tools/browser-helper/browser_helper/ ./browser_helper/
COPY tools/browser-helper/tests/ ./tests/
RUN uv run --offline --frozen --no-sync python -B -m unittest discover -s tests

FROM python:3.13-slim-bookworm AS browser
ARG TARGETARCH
RUN test "$TARGETARCH" = amd64
# Debian security updates intentionally follow Bookworm; app dependencies are locked.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# Official unmodified archive, pinned to the existing Nix upstream version/hash.
RUN curl --fail --show-error --location \
      https://cloakbrowser.dev/chromium-v146.0.7680.177.5/cloakbrowser-linux-x64.tar.gz \
      --output /tmp/browser.tar.gz \
    && printf '%s  %s\n' 4a12bcde95fa1bb1beef2b41ab5e5c27c36be78e3be3d0dac8c64d705216670e /tmp/browser.tar.gz | sha256sum --check --strict \
    && mkdir -p /opt/cloakbrowser \
    && tar -xzf /tmp/browser.tar.gz -C /opt/cloakbrowser \
    && test -x /opt/cloakbrowser/chrome \
    && rm /tmp/browser.tar.gz
RUN curl --fail --show-error --location \
      https://raw.githubusercontent.com/CloakHQ/CloakBrowser/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/BINARY-LICENSE.md \
      --output /opt/cloakbrowser/PROJECT-BINARY-LICENSE.md \
    && curl --fail --show-error --location \
      https://raw.githubusercontent.com/CloakHQ/CloakBrowser/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/LICENSE \
      --output /opt/cloakbrowser/PROJECT-WRAPPER-LICENSE

FROM python:3.13-slim-bookworm AS runtime
# The label describes original application code, not the separately licensed browser.
LABEL org.opencontainers.image.title="AnyRouter Manager" \
      org.opencontainers.image.licenses="GPL-3.0-or-later" \
      dev.anyrouter.browser-license="CloakBrowser Binary License; internal-use image only"
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates tini util-linux fonts-liberation fonts-noto-cjk fonts-noto-color-emoji \
      libasound2 libatk-bridge2.0-0 libatk1.0-0 libatspi2.0-0 libcairo2 libcups2 \
      libdbus-1-3 libdrm2 libexpat1 libfontconfig1 libfreetype6 libgbm1 \
      libgdk-pixbuf-2.0-0 libglib2.0-0 libgtk-3-0 libnspr4 libnss3 libpango-1.0-0 \
      libpulse0 libwayland-client0 libx11-6 libxcb1 libxcomposite1 libxcursor1 \
      libxdamage1 libxext6 libxfixes3 libxi6 libxkbcommon0 libxrandr2 libxrender1 \
      libxshmfence1 libxss1 libxtst6 libgl1 libegl1 libstdc++6 libgcc-s1 \
      adwaita-icon-theme gsettings-desktop-schemas xdg-utils \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 anyrouter \
    && useradd --uid 1000 --gid 1000 --no-create-home --home-dir /tmp/anyrouter anyrouter \
    && mkdir -p /app /config /data \
    && chown 1000:1000 /data && chmod 0700 /data
ENV HOME=/tmp/anyrouter \
    PYTHONDONTWRITEBYTECODE=1 \
    UV_PYTHON_DOWNLOADS=never \
    UV_CACHE_DIR=/tmp/anyrouter/uv-cache \
    CLOAKBROWSER_BINARY_PATH=/opt/cloakbrowser/chrome \
    CLOAKBROWSER_AUTO_UPDATE=false \
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
WORKDIR /app
COPY --from=backend /build/backend/target/release/anyrouter-manager-backend /app/anyrouter-manager-backend
COPY --from=frontend /build/frontend/dist/ /app/frontend/dist/
COPY --from=helper /app/tools/browser-helper/ /app/tools/browser-helper/
COPY --from=helper /usr/local/bin/uv /usr/local/bin/uv
COPY --from=browser /opt/cloakbrowser/ /opt/cloakbrowser/
COPY LICENSE THIRD_PARTY_NOTICES.md /app/
COPY docker/entrypoint.sh docker/healthcheck.py /app/docker/
USER 1000:1000
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["python", "-B", "/app/docker/healthcheck.py"]
ENTRYPOINT ["/usr/bin/tini", "--", "/bin/sh", "/app/docker/entrypoint.sh"]
CMD ["/app/anyrouter-manager-backend", "--config", "/config/config.toml"]
