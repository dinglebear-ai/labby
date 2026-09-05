# check=skip=InvalidDefaultArgInFrom
# ─── Stage 1: Build ───────────────────────────────────────────────────────────
# Requires Rust 1.97.1+ (Cargo.toml: rust-version = "1.97.1", edition = "2024")
# Supply-chain policy requires callers to pass a versioned, digest-pinned image.
ARG LABBY_BUILDER_IMAGE
ARG LABBY_RUNTIME_IMAGE
ARG LABBY_DEBIAN_SNAPSHOT
FROM ${LABBY_BUILDER_IMAGE} AS builder
ARG LABBY_DEBIAN_SNAPSHOT

WORKDIR /build

# GitHub's container builder has less memory than the release-smoke runners.
# Keep the container release binary optimized but avoid ThinLTO/codegen-units=1
# peak memory during the final link.
ENV CARGO_PROFILE_RELEASE_LTO=false \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16

# System build deps:
#   pkg-config     — lets -sys crates find system headers
#   build-essential — C compiler for rusqlite's bundled SQLite + other -sys crates
#   libssl-dev     — headers/pkg-config metadata for crates pulling openssl-sys
#   libclang-dev   — libclang for rquickjs-sys bindgen during Code Mode builds
RUN printf '%s\n' \
      "deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/${LABBY_DEBIAN_SNAPSHOT}/ trixie main" \
      "deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/${LABBY_DEBIAN_SNAPSHOT}/ trixie-security main" \
      > /etc/apt/sources.list \
 && rm -f /etc/apt/sources.list.d/debian.sources \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      build-essential \
      libssl-dev \
      libclang-dev \
 && rm -rf /var/lib/apt/lists/*

# ── Dependency-caching layer ──────────────────────────────────────────────────
# Copy only manifests so the heavy dep-compile layer is invalidated only when
# Cargo.toml / Cargo.lock changes, not on every src edit.
COPY Cargo.toml Cargo.lock ./
COPY crates/labby/Cargo.toml      crates/labby/Cargo.toml
COPY crates/labby-apis/Cargo.toml crates/labby-apis/Cargo.toml
COPY crates/labby-auth/Cargo.toml crates/labby-auth/Cargo.toml
COPY crates/labby-runtime/Cargo.toml crates/labby-runtime/Cargo.toml
COPY crates/labby-codemode/Cargo.toml crates/labby-codemode/Cargo.toml
COPY crates/labby-gateway/Cargo.toml crates/labby-gateway/Cargo.toml
COPY crates/labby-web/Cargo.toml crates/labby-web/Cargo.toml
COPY crates/labby-winjob/Cargo.toml crates/labby-winjob/Cargo.toml
COPY crates/labby-primitives/Cargo.toml crates/labby-primitives/Cargo.toml
COPY crates/labby-openapi/Cargo.toml crates/labby-openapi/Cargo.toml
COPY crates/xtask/Cargo.toml crates/xtask/Cargo.toml

# Create stub source files so cargo can parse ALL manifests without error.
#
# labby has three explicit [[bin]] targets plus a [lib] target — stub all four.
#
# We also need apps/gateway-admin/out/ to exist for include_dir!() in web.rs,
# but we copy the real one below — for the dep-cache step a stub dir suffices.
RUN mkdir -p \
      crates/labby/src \
      crates/labby-apis/src \
      crates/labby-auth/src \
      crates/labby-runtime/src \
      crates/labby-codemode/src \
      crates/labby-gateway/src \
      crates/labby-web/src \
      crates/labby-winjob/src \
      crates/labby-primitives/src \
      crates/labby-openapi/src \
      crates/labby/tests/fixtures \
      crates/xtask/src \
      apps/gateway-admin/out \
 && echo 'fn main(){}' > crates/labby/src/main.rs \
 && echo 'fn main(){}' > crates/labby/tests/fixtures/stdio_mcp_fixture.rs \
 && echo 'fn main(){}' > crates/labby/tests/fixtures/live_harness_fixture.rs \
 && echo 'fn main(){}' > crates/xtask/src/main.rs \
 && echo 'fn main(){}' > crates/labby-web/build.rs \
 && touch \
      crates/labby/src/lib.rs \
      crates/labby-apis/src/lib.rs \
      crates/labby-auth/src/lib.rs \
      crates/labby-runtime/src/lib.rs \
      crates/labby-codemode/src/lib.rs \
      crates/labby-gateway/src/lib.rs \
      crates/labby-web/src/lib.rs \
      crates/labby-winjob/src/lib.rs \
      crates/labby-primitives/src/lib.rs \
      crates/labby-openapi/src/lib.rs \
 && cargo build --workspace --all-features --release \
 && cargo clean -p labby \
 && cargo clean -p labby-apis \
 && cargo clean -p labby-auth \
 && cargo clean -p labby-runtime \
 && cargo clean -p labby-codemode \
 && cargo clean -p labby-gateway \
 && cargo clean -p labby-web \
 && cargo clean -p labby-winjob \
 && cargo clean -p labby-primitives \
 && cargo clean -p labby-openapi \
 && cargo clean -p xtask \
 && rm -rf \
      crates/*/src \
      apps/gateway-admin/out \
      target/release/labby

# ── Real source + frontend assets ─────────────────────────────────────────────
# crates/labby/build.rs embeds the Next.js static export (apps/gateway-admin/out)
# into the binary via include_bytes!. The out/ dir must exist before `cargo build`.
# (.dockerignore excludes apps/gateway-admin/{node_modules,.next,...} — only
# apps/gateway-admin/out is sent in the build context.)
COPY apps/gateway-admin/out apps/gateway-admin/out
COPY docs/generated/env-reference.json docs/generated/env-reference.json
COPY docs/contracts/ docs/contracts/
COPY plugins/labby/skills/using-labby/ plugins/labby/skills/using-labby/
COPY plugins/labby/skills/creating-snippets/ plugins/labby/skills/creating-snippets/
COPY scripts/incus-bootstrap.sh scripts/incus-bootstrap.sh
COPY scripts/install.sh scripts/install.sh
COPY config/incus/labby-backup.yaml config/incus/labby-backup.yaml
COPY config/incus/labby-gateway-profile.yaml config/incus/labby-gateway-profile.yaml
COPY config/incus/labby-image.yaml config/incus/labby-image.yaml
COPY crates/ crates/
# BuildKit normalizes COPYed file mtimes to a fixed past epoch. That timestamp is
# older than the stub rlibs built in the dep-cache layer above, so cargo's
# fingerprint considers the real workspace sources "unchanged" and skips
# rebuilding them — linking the empty stub crates (esp. the patched
# workspace dependency stubs that Cargo did not clean) and producing hundreds of
# "cannot find X in labby_apis/labby_auth" errors. Touch the
# real sources so their mtimes are newer, forcing a from-source rebuild of every
# workspace crate while keeping the cached third-party dependency artifacts.
RUN find crates -type f -exec touch {} + \
 && cargo build --workspace --all-features --release

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
# Supply-chain policy requires callers to pass a versioned, digest-pinned image.
FROM ${LABBY_RUNTIME_IMAGE} AS runtime

ARG LABBY_NPM_VERSION
ARG LABBY_NPM_SHA512
ARG LABBY_NPM_BRACE_EXPANSION_VERSION
ARG LABBY_NPM_BRACE_EXPANSION_SHA512
ARG LABBY_NPM_IP_ADDRESS_VERSION
ARG LABBY_NPM_IP_ADDRESS_SHA512
ARG LABBY_DEBIAN_SNAPSHOT
ARG LABBY_NODE_VERSION
ARG LABBY_NODE_SHA256
ARG LABBY_UV_VERSION
ARG LABBY_UV_SHA256
ARG LABBY_AGENT_CLIS_LOCK_SHA256

LABEL io.modelcontextprotocol.server.name="ai.dinglebear/labby"

# ca-certificates: reqwest uses rustls for TLS but still validates against the
# system CA bundle — required for outbound HTTPS calls.
# git remains available for operator-managed stdio upstreams and setup workflows.
# Explicitly install the util-linux package family so a floating Debian base is
# upgraded to the current security candidates before the release image is scanned.
# Replace NodeSource's bundled npm from a digest-pinned tarball, then patch
# fixed bundled dependencies that have not yet landed in an npm release.
RUN printf '%s\n' \
      "deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/${LABBY_DEBIAN_SNAPSHOT}/ trixie main" \
      "deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/${LABBY_DEBIAN_SNAPSHOT}/ trixie-security main" \
      > /etc/apt/sources.list \
 && rm -f /etc/apt/sources.list.d/debian.sources \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git \
      bsdutils libblkid1 liblastlog2-2 libmount1 libsmartcols1 libuuid1 \
      login mount util-linux xz-utils zsh \
 && curl -fsSLo /tmp/node.tar.xz --connect-timeout 10 --max-time 300 "https://nodejs.org/dist/v${LABBY_NODE_VERSION}/node-v${LABBY_NODE_VERSION}-linux-x64.tar.xz" \
 && echo "${LABBY_NODE_SHA256}  /tmp/node.tar.xz" | sha256sum --check --strict \
 && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
 && rm /tmp/node.tar.xz \
 && npm_root="$(npm root --global)" \
 && curl -fsSLo /tmp/npm.tgz --connect-timeout 10 --max-time 300 "https://registry.npmjs.org/npm/-/npm-${LABBY_NPM_VERSION}.tgz" \
 && echo "${LABBY_NPM_SHA512}  /tmp/npm.tgz" | sha512sum --check --strict \
 && mkdir -p /tmp/npm-package \
 && tar -xzf /tmp/npm.tgz --strip-components=1 -C /tmp/npm-package \
 && rm -rf "${npm_root}/npm" \
 && mv /tmp/npm-package "${npm_root}/npm" \
 && curl -fsSLo /tmp/brace-expansion.tgz --connect-timeout 10 --max-time 300 "https://registry.npmjs.org/brace-expansion/-/brace-expansion-${LABBY_NPM_BRACE_EXPANSION_VERSION}.tgz" \
 && echo "${LABBY_NPM_BRACE_EXPANSION_SHA512}  /tmp/brace-expansion.tgz" | sha512sum --check --strict \
 && rm -rf "${npm_root}/npm/node_modules/brace-expansion" \
 && mkdir -p "${npm_root}/npm/node_modules/brace-expansion" \
 && tar -xzf /tmp/brace-expansion.tgz --strip-components=1 -C "${npm_root}/npm/node_modules/brace-expansion" \
 && curl -fsSLo /tmp/ip-address.tgz --connect-timeout 10 --max-time 300 "https://registry.npmjs.org/ip-address/-/ip-address-${LABBY_NPM_IP_ADDRESS_VERSION}.tgz" \
 && echo "${LABBY_NPM_IP_ADDRESS_SHA512}  /tmp/ip-address.tgz" | sha512sum --check --strict \
 && rm -rf "${npm_root}/npm/node_modules/ip-address" \
 && mkdir -p "${npm_root}/npm/node_modules/ip-address" \
 && tar -xzf /tmp/ip-address.tgz --strip-components=1 -C "${npm_root}/npm/node_modules/ip-address" \
 && ln -sf "${npm_root}/npm/bin/npm-cli.js" /usr/local/bin/npm \
 && ln -sf "${npm_root}/npm/bin/npx-cli.js" /usr/local/bin/npx \
 && test "$(npm --version)" = "${LABBY_NPM_VERSION}" \
 && test "$(node -p "require('${npm_root}/npm/node_modules/tar/package.json').version")" = "7.5.19" \
 && test "$(node -p "require('${npm_root}/npm/node_modules/undici/package.json').version")" = "6.27.0" \
 && test "$(node -p "require('${npm_root}/npm/node_modules/brace-expansion/package.json').version")" = "${LABBY_NPM_BRACE_EXPANSION_VERSION}" \
 && test "$(node -p "require('${npm_root}/npm/node_modules/ip-address/package.json').version")" = "${LABBY_NPM_IP_ADDRESS_VERSION}" \
 && rm -f /tmp/npm.tgz /tmp/brace-expansion.tgz /tmp/ip-address.tgz \
 && rm -rf /var/lib/apt/lists/*

# Agent CLIs used by stdio MCP servers that shell out to provider tools.
# Keep these deterministic so container behavior matches local development.
# npm 12 blocks unapproved dependency install scripts; run Claude Code's pinned
# postinstall explicitly so its platform-native optional binary is installed.
COPY config/agent-clis/package.json config/agent-clis/package-lock.json /opt/agent-clis/
RUN mkdir -p /opt/agent-clis \
 && cd /opt/agent-clis \
 && echo "${LABBY_AGENT_CLIS_LOCK_SHA256}  package-lock.json" | sha256sum --check --strict \
 && npm ci --omit=dev --no-audit --no-fund \
 && node node_modules/@anthropic-ai/claude-code/install.cjs \
 && ln -sf /opt/agent-clis/node_modules/.bin/claude /usr/local/bin/claude \
 && ln -sf /opt/agent-clis/node_modules/.bin/codex /usr/local/bin/codex \
 && npm cache clean --force \
 && claude --version \
 && codex --version

# Python MCP servers commonly use `uvx` as their stdio launcher. Install uv
# into PATH for gateway child processes.
#
RUN curl -fsSLo /tmp/uv.tar.gz --connect-timeout 10 --max-time 300 "https://github.com/astral-sh/uv/releases/download/${LABBY_UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz" \
 && echo "${LABBY_UV_SHA256}  /tmp/uv.tar.gz" | sha256sum --check --strict \
 && tar -xzf /tmp/uv.tar.gz -C /tmp \
 && install -m 0755 /tmp/uv-x86_64-unknown-linux-gnu/uv /usr/local/bin/uv \
 && install -m 0755 /tmp/uv-x86_64-unknown-linux-gnu/uvx /usr/local/bin/uvx \
 && rm -rf /tmp/uv.tar.gz /tmp/uv-x86_64-unknown-linux-gnu \
 && uv --version \
 && uvx --version

# Pre-create XDG directories so the binary never needs mkdir at runtime.
#   ~/.local/state/labby  — master.lock and runtime state
#   ~/.local/share/labby  — SQLite databases (node log, registry, auth)
#   ~/.config/labby       — config.toml (mounted read-only from host)
RUN mkdir -p \
      /home/labby/.config/labby \
      /home/labby/.local/state/labby \
      /home/labby/.local/share/labby \
 && chown -R 1000:1000 /home/labby

COPY --from=builder /build/target/release/labby /usr/local/bin/labby

# Health recovery has one explicit owner. After three failed probes the helper
# records redacted evidence in the persistent state volume and terminates PID 1.
# It exponentially backs off restart requests and stops requesting replacement
# after the bounded storm threshold, leaving the container unhealthy for review.
COPY --chmod=0755 scripts/ci/container-healthcheck.sh /usr/local/bin/labby-healthcheck

ENV HOME=/home/labby
USER 1000:1000
WORKDIR /home/labby

# Default HTTP port. Override with LABBY_MCP_HTTP_PORT or config.toml.
EXPOSE 8765

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/usr/local/bin/labby-healthcheck"]

ENTRYPOINT ["/usr/local/bin/labby"]
# Master HTTP mode by default.
# Override CMD to ["serve","mcp","--stdio"] for the stdio-only MCP node.
CMD ["serve"]
