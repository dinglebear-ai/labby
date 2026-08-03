# ─── Stage 1: Build ───────────────────────────────────────────────────────────
# Requires Rust 1.97.1+ (Cargo.toml: rust-version = "1.97.1", edition = "2024")
# TODO: pin by digest (FROM rust:1-slim@sha256:...) for reproducible builds —
#       floating tag resolves a fresh image per build.
FROM rust:1-slim AS builder

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
RUN apt-get update \
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
# labby has both [[bin]] (main.rs) and [lib] (lib.rs) targets — stub both.
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
      crates/xtask/src \
      apps/gateway-admin/out \
 && echo 'fn main(){}' > crates/labby/src/main.rs \
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
# TODO: pin by digest (FROM debian:trixie-slim@sha256:...) for reproducible
#       builds — floating tag resolves a fresh image per build.
FROM debian:trixie-slim AS runtime

ARG NPM_VERSION="12.0.2"
ARG NPM_SHA512="b885e890b9418fa1693544d05f53e64f9a73ec194837d4258b15fecdd692347b1dd2a517b1b0cbaf9d31cd8e92c3b70956bd2ecc72833a57b4b3098f5bfa7943"

LABEL io.modelcontextprotocol.server.name="ai.dinglebear/labby"

# ca-certificates: reqwest uses rustls for TLS but still validates against the
# system CA bundle — required for outbound HTTPS calls.
# git remains available for operator-managed stdio upstreams and setup workflows.
# NodeSource's bundled npm carries vulnerable nested tar/undici copies.
# Replace it from a digest-pinned tarball without asking npm to overwrite itself.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git gnupg \
 && mkdir -p /etc/apt/keyrings \
 && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
    | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
 && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_24.x nodistro main" \
    > /etc/apt/sources.list.d/nodesource.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends nodejs zsh \
 && npm_root="$(npm root --global)" \
 && curl -fsSLo /tmp/npm.tgz "https://registry.npmjs.org/npm/-/npm-${NPM_VERSION}.tgz" \
 && echo "${NPM_SHA512}  /tmp/npm.tgz" | sha512sum --check --strict \
 && mkdir -p /tmp/npm-package \
 && tar -xzf /tmp/npm.tgz --strip-components=1 -C /tmp/npm-package \
 && rm -rf "${npm_root}/npm" \
 && mv /tmp/npm-package "${npm_root}/npm" \
 && ln -sf "${npm_root}/npm/bin/npm-cli.js" /usr/local/bin/npm \
 && ln -sf "${npm_root}/npm/bin/npx-cli.js" /usr/local/bin/npx \
 && test "$(npm --version)" = "${NPM_VERSION}" \
 && test "$(node -p "require('${npm_root}/npm/node_modules/tar/package.json').version")" = "7.5.19" \
 && test "$(node -p "require('${npm_root}/npm/node_modules/undici/package.json').version")" = "6.27.0" \
 && rm -f /tmp/npm.tgz \
 && rm -rf /var/lib/apt/lists/*

# Agent CLIs used by stdio MCP servers that shell out to provider tools.
# Keep these deterministic so container behavior matches local development.
RUN mkdir -p /opt/agent-clis \
 && cd /opt/agent-clis \
 && npm init -y >/dev/null \
 && npm install --omit=dev --no-audit --no-fund \
      @anthropic-ai/claude-code@2.1.132 \
      @openai/codex@0.130.0 \
 # npm 12 blocks unapproved dependency install scripts; run Claude Code's pinned
 # postinstall explicitly so its platform-native optional binary is installed.
 && node node_modules/@anthropic-ai/claude-code/install.cjs \
 && ln -sf /opt/agent-clis/node_modules/.bin/claude /usr/local/bin/claude \
 && ln -sf /opt/agent-clis/node_modules/.bin/codex /usr/local/bin/codex \
 && npm cache clean --force \
 && claude --version \
 && codex --version

# Python MCP servers commonly use `uvx` as their stdio launcher. Install uv
# into PATH for gateway child processes.
#
# Reproducibility: the astral.sh installer pins a version when UV_VERSION is set
# (it fetches that exact release instead of "latest"). UV_VERSION is currently
# empty, so this tracks the latest uv at build time.
# TODO: pin uv by setting UV_VERSION to a verified release tag (e.g. "0.x.y")
#       so the runtime image is reproducible across builds.
ARG UV_VERSION=""
RUN curl -LsSf https://astral.sh/uv/install.sh \
    | env UV_INSTALL_DIR=/usr/local/bin UV_VERSION="${UV_VERSION}" sh \
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

ENV HOME=/home/labby
USER 1000:1000
WORKDIR /home/labby

# Default HTTP port.  Override with LAB_MCP_HTTP_PORT env var or config.toml.
EXPOSE 8765

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8765/health || exit 1

ENTRYPOINT ["/usr/local/bin/labby"]
# Master HTTP mode by default.
# Override CMD to ["serve","mcp","--stdio"] for the stdio-only MCP node.
CMD ["serve"]
