# syntax=docker/dockerfile:1.7
#
# Zunder container image: web UI + zunder-server (+ zunder-import) in one image.
#
# Multi-arch strategy (linux/amd64 and linux/arm64):
#
#   The web and Rust builder stages run on the *build* platform
#   (`--platform=$BUILDPLATFORM`) and cross-compile for the target platform.
#   Compiling Rust under QEMU emulation is 5-20x slower and occasionally
#   crashes (rustc/LLVM under qemu-user), so emulation is only used for the
#   tiny runtime stage (apt-get + useradd). rusqlite bundles SQLite and needs
#   a C compiler for the target, so the builder installs the Debian cross gcc
#   for the target architecture when it differs from the build architecture.
#   Builder and runtime are both Debian bookworm, so the glibc versions match.
#
#   Requirement for the server code: no OpenSSL / native-tls dependencies
#   (use rustls), otherwise cross builds would also need target libssl-dev.
#
# Dependency caching: cargo-chef compiles all external crates in their own
# layer, keyed only on Cargo.toml/Cargo.lock, so source changes rebuild only
# the workspace crates.
#
# Build locally:
#   docker build -t zunder .
#   docker buildx build --platform linux/amd64,linux/arm64 -t zunder .

ARG RUST_VERSION=1.97
ARG NODE_VERSION=22
ARG DEBIAN_RELEASE=bookworm

# ---------------------------------------------------------------------------
# Stage 1: web UI (platform independent output, built natively)
# ---------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-${DEBIAN_RELEASE}-slim AS web
WORKDIR /src/web
# Browsers are only needed for Playwright tests, never inside the image.
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# ---------------------------------------------------------------------------
# Stage 2a: Rust toolchain with cargo-chef (identical for all targets)
# ---------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-${DEBIAN_RELEASE} AS chef
RUN cargo install cargo-chef --locked --version 0.1.78
WORKDIR /src
# Same settings for `chef cook` and `cargo build`, otherwise the cached
# dependencies would be fingerprinted differently and rebuilt.
ENV CARGO_PROFILE_RELEASE_STRIP=symbols \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar

# ---------------------------------------------------------------------------
# Stage 2b: dependency recipe (only manifests matter for the cache key)
# ---------------------------------------------------------------------------
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo chef prepare --recipe-path recipe.json

# ---------------------------------------------------------------------------
# Stage 2c: cross toolchain for the target, cached dependencies, binaries
# ---------------------------------------------------------------------------
FROM chef AS builder
ARG TARGETARCH
# Map the Docker architecture to the Rust target triple. The native gcc of
# the rust image already provides `<triple>-gcc`; the Debian cross packages
# are only installed when the target architecture differs.
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-gnu;  cross="gcc-x86-64-linux-gnu libc6-dev-amd64-cross" ;; \
      arm64) target=aarch64-unknown-linux-gnu; cross="gcc-aarch64-linux-gnu libc6-dev-arm64-cross" ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    if [ "$(dpkg --print-architecture)" != "$TARGETARCH" ]; then \
      apt-get update; \
      apt-get install -y --no-install-recommends $cross; \
      rm -rf /var/lib/apt/lists/*; \
    fi; \
    rustup target add "$target"; \
    echo "$target" > /rust-target

COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --target "$(cat /rust-target)" --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN set -eux; \
    target="$(cat /rust-target)"; \
    cargo build --release --locked --target "$target" --bin zunder-server --bin zunder-import; \
    mkdir -p /out; \
    cp "target/$target/release/zunder-server" "target/$target/release/zunder-import" /out/

# ---------------------------------------------------------------------------
# Stage 3: runtime
# ---------------------------------------------------------------------------
FROM debian:${DEBIAN_RELEASE}-slim AS runtime
# ca-certificates: outgoing HTTPS (Pocket-ID, Timing)
# curl:            HEALTHCHECK against /healthz
# sqlite3:         integrity checks and manual backups/restores (docs/betrieb.md)
# tini:            PID 1, forwards SIGTERM and reaps zombies
# tzdata:          local time for ZUNDER_BACKUP_HOUR (set TZ, e.g. Europe/Berlin)
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl sqlite3 tini tzdata; \
    rm -rf /var/lib/apt/lists/*; \
    groupadd --system --gid 10001 zunder; \
    useradd --system --uid 10001 --gid zunder --home-dir /data --no-create-home \
      --shell /usr/sbin/nologin zunder; \
    mkdir -p /data /app/web; \
    chown zunder:zunder /data

COPY --from=web /src/web/dist /app/web
COPY --from=builder /out/zunder-server /out/zunder-import /usr/local/bin/

ENV ZUNDER_BIND=0.0.0.0:8080 \
    ZUNDER_DATA_DIR=/data \
    ZUNDER_WEB_DIR=/app/web

USER zunder
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8080

# The health check assumes the default port; adjust it when ZUNDER_BIND changes.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["zunder-server"]
