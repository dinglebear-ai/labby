# Labby — Development Commands

local_release_profile := "release-fast"

default:
    @just --list

# Check all crates compile
check:
    cargo check --workspace --all-features

# Run all tests
test:
    cargo nextest run --workspace --all-features

# Regenerate code-owned documentation inventories
docs-generate:
    cargo run --package labby --bin labby --all-features -- docs generate

# Verify generated documentation inventories are fresh and maintained local links resolve
docs-check:
    cargo run --package labby --bin labby --all-features -- docs check
    python3 scripts/check-doc-links.py
    python3 scripts/check-product-docs.py
    python3 scripts/check-depot-control-plane-contract.py
    python3 -m unittest scripts/ci/test_depot_control_plane_contract.py

# Build strict Rustdoc for the complete workspace target surface.
rustdoc:
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -D warnings --document-private-items"
    # Default targets cover every workspace library plus binary-only packages.
    cargo doc --workspace --all-features --no-deps --document-private-items --locked
    # Cargo omits secondary bins/examples when a package also has a library.
    # The six-line `labby` launcher shares the library crate name and cannot be
    # published without Cargo issue #6313 overwriting one of the pages; its real
    # API is `labby::run()`. Document the non-colliding fixture + examples here.
    cargo rustdoc -p labby --all-features --bin stdio-mcp-fixture --examples --locked --target-dir target/rustdoc-extra

# Build strict Rustdoc and execute all workspace doctests.
rustdoc-check: rustdoc
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -D warnings"
    cargo test --doc --workspace --all-features --locked

# Report missing public API prose without making historical coverage debt block CI.
rustdoc-audit:
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTDOCFLAGS="${RUSTDOCFLAGS:-} --force-warn missing_docs"
    cargo doc --workspace --all-features --no-deps --document-private-items --locked

# Run integration tests (requires running services)
test-integration:
    cargo nextest run --workspace --all-features --run-ignored ignored-only

# Run the pinned, isolated real-Authelia OIDC acceptance harness.
test-authelia:
    bash tests/authelia/run.sh

live-e2e tier="pr" seed="1":
    scripts/ci/labby-live-e2e.sh "{{tier}}" "{{seed}}"

# Lint
lint: skill-drift rust-toolchain-sync module-reachability
    # --all-targets so tests/examples/benches are linted too. Without it the
    # `disallowed_methods` bans (Tool::new, Peer::list_all_*) do not cover test
    # code, which is exactly where fixtures reach for them.
    cargo clippy --workspace --all-features --all-targets -- -D warnings
    cargo fmt --all -- --check

# Fail when a file under `crates/*/src` exists on disk that no parent module
# declares — or a whole directory that no sibling module file can declare.
# rustc never sees such a file, so it produces no warning and any tests inside
# it silently stop running — the failure mode that orphaned `paginate.rs` and
# two test modules for months. Rust crates outside `crates/` are not walked.
module-reachability:
    cargo test -p xtask module_reachability

# Verify Cargo, rust-toolchain, CI, container, and active docs agree on the MSRV.
rust-toolchain-sync:
    bash scripts/check-rust-toolchain-sync.sh

# Check hand-authored skills for known stale or unsafe patterns
skill-drift:
    LAB_ALLOW_MISSING_DOZZLE=1 plugins/scripts/check-dozzle-skill

# License and vulnerability audit
deny:
    cargo deny check

# Build with all features using the release-fast profile (optimized, no LTO/codegen-units=1
# slowdown). Use `cargo build --workspace --all-features` directly for a debug-assertions/
# full-unwind dev build instead.
build:
    cargo build --workspace --all-features --profile {{local_release_profile}}

# Build release binary with all features.
# bin/labby is the container bind-mount (docker-compose.yml); the plugin does
# NOT ship a binary — hosts install labby via scripts/install.sh or cargo.
build-release:
    cargo build --workspace --all-features --release
    mkdir -p bin
    install -m 755 target/release/labby bin/labby
    just link-bin

# Copy the compiled binary into PATH.
# Called automatically by `just build-release` and `just install`.
link-bin profile="release":
    #!/usr/bin/env bash
    set -euo pipefail
    if systemctl is-active --quiet labby.service 2>/dev/null; then
      echo "error: labby.service is active; use 'just host-sync' so the service restarts onto the new binary" >&2
      exit 1
    fi
    profile="{{profile}}"
    just _install-labby-bin "$profile"

_install-labby-bin profile:
    #!/usr/bin/env bash
    set -euo pipefail
    profile="{{profile}}"
    LABBY_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
    case "$LABBY_TARGET_DIR" in
      /*) LABBY_BIN="$LABBY_TARGET_DIR/$profile/labby" ;;
      *)  LABBY_BIN="$(pwd)/$LABBY_TARGET_DIR/$profile/labby" ;;
    esac
    if [ ! -x "$LABBY_BIN" ]; then
      echo "$profile binary not found at $LABBY_BIN — run the matching build first" >&2
      exit 1
    fi
    local_bin_dir="${HOME}/.local/bin"
    mkdir -p "$local_bin_dir"
    if [ -x "$local_bin_dir/labby" ]; then
      cp -f "$local_bin_dir/labby" "$local_bin_dir/labby.prev"
    fi
    install -m 755 "$LABBY_BIN" "$local_bin_dir/labby.new"
    mv "$local_bin_dir/labby.new" "$local_bin_dir/labby"
    echo "labby → $LABBY_BIN"

# Build release-fast binary, copy it to the system service path, and restart the
# system Labby gateway service. The primary self-hosted runtime is the Incus
# system-container path; this source checkout shortcut assumes sudo access.
host-sync:
    #!/usr/bin/env bash
    set -euo pipefail
    profile="{{local_release_profile}}"
    if command -v mold >/dev/null 2>&1; then
      export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=mold"
    fi
    cargo build --workspace --all-features --profile "$profile" --bin labby
    LABBY_BIN="target/$profile/labby"
    sudo mkdir -p /usr/local/bin
    sudo install -m 755 "$LABBY_BIN" /usr/local/bin/labby
    if systemctl is-active --quiet labby.service; then
      sudo /usr/local/bin/labby setup host-service restart -y
      sudo /usr/local/bin/labby setup host-service status --json
    else
      echo "error: labby.service is not active; run: just host-service-install" >&2
      exit 1
    fi

# Benchmark the gateway-host feature slice used by the normal hosted gateway
# deployment: gateway + web UI + OAuth, without optional all-features extras.
bench-slim clean="":
    #!/usr/bin/env bash
    set -euo pipefail
    args=()
    if [ "{{clean}}" = "clean" ]; then
      args+=(--clean)
    fi
    scripts/bench-labby-slimming "${args[@]}"

host-service-install:
    #!/usr/bin/env bash
    set -euo pipefail
    profile="{{local_release_profile}}"
    if command -v mold >/dev/null 2>&1; then
      export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=mold"
    fi
    cargo build --workspace --all-features --profile "$profile" --bin labby
    LABBY_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
    case "$LABBY_TARGET_DIR" in
      /*) LABBY_BIN="$LABBY_TARGET_DIR/$profile/labby" ;;
      *)  LABBY_BIN="$(pwd)/$LABBY_TARGET_DIR/$profile/labby" ;;
    esac
    sudo mkdir -p /usr/local/bin
    sudo install -m 755 "$LABBY_BIN" /usr/local/bin/labby
    sudo /usr/local/bin/labby setup host-service install -y

host-service-restart:
    sudo /usr/local/bin/labby setup host-service restart -y
    sudo /usr/local/bin/labby setup host-service status --json

host-service-status:
    sudo /usr/local/bin/labby setup host-service status --json

host-service-uninstall:
    sudo /usr/local/bin/labby setup host-service uninstall -y

# Explicit container compatibility path. Prefer host-sync for normal gateway
# development; this remains useful for prod-like image smoke and Docker-specific
# ACP adapter changes.
dev-container: web-build build-release
    docker compose -f docker-compose.yml restart

dev-container-debug:
    #!/usr/bin/env bash
    set -euo pipefail
    nightly_rustc=$(rustup which --toolchain nightly rustc)
    RUSTC="$nightly_rustc" RUSTC_WRAPPER="" RUSTFLAGS="-C link-arg=-fuse-ld=mold -Z codegen-backend=cranelift" \
        cargo build -p labby --all-features
    mkdir -p bin
    install -m 755 target/debug/labby bin/labby
    docker compose -f docker-compose.yml restart

# Explicit container sync path. The normal gateway workflow is host-sync.
# Rebuilds the dev image only when runtime inputs changed, then restarts Docker.
sync-container:
    #!/usr/bin/env bash
    set -euo pipefail
    repo="$(pwd)"
    profile="{{local_release_profile}}"
    if command -v mold >/dev/null 2>&1; then
      export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=mold"
    fi

    LABBY_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
    case "$LABBY_TARGET_DIR" in
      /*) LABBY_BIN="$LABBY_TARGET_DIR/$profile/labby" ;;
      *)  LABBY_BIN="$repo/$LABBY_TARGET_DIR/$profile/labby" ;;
    esac

    release_stale=0
    if [ ! -x "$LABBY_BIN" ]; then
      release_stale=1
    else
      while IFS= read -r -d '' input; do
        if [ "$input" -nt "$LABBY_BIN" ]; then
          release_stale=1
          break
        fi
      done < <(git ls-files -z -- Cargo.toml Cargo.lock rust-toolchain.toml .cargo build.rs crates config apps/gateway-admin/out)
    fi
    if [ "$release_stale" -eq 1 ]; then
      cargo build --workspace --all-features --profile "$profile" --bin labby
    else
      echo "$profile binary is current: $LABBY_BIN"
    fi

    mkdir -p bin
    install -m 755 "$LABBY_BIN" bin/labby
    mkdir -p ~/.local/bin
    ln -sf "$LABBY_BIN" ~/.local/bin/labby
    echo "labby → $LABBY_BIN"

    compose=(docker compose -f docker-compose.yml)
    container_sentinel="$LABBY_TARGET_DIR/.labby-container-built"
    image_stale=0
    if ! docker image inspect labby:dev >/dev/null 2>&1; then
      image_stale=1
    else
      while IFS= read -r -d '' input; do
        if [ "$input" -nt "$container_sentinel" ] 2>/dev/null; then
          image_stale=1
          break
        fi
      done < <(git ls-files -z -- config/Dockerfile.fast docker-compose.yml docker-compose.prod.yml config/acp-adapters.package.json)
    fi
    if [ "$image_stale" -eq 1 ]; then
      "${compose[@]}" build labby-master
      mkdir -p "$(dirname "$container_sentinel")"
      touch "$container_sentinel"
      "${compose[@]}" up -d labby-master --no-deps --no-build
    else
      echo "dev runtime image is current"
      "${compose[@]}" up -d labby-master --no-deps --no-build
    fi
    "${compose[@]}" restart labby-master
    "${compose[@]}" ps labby-master
    echo "container synced"

container-sync: sync-container

# Install release binary to ~/.local/bin/labby (updates the host CLI)
install: build-release
    just link-bin

# Build, install, and keep the local macOS gateway alive through launchd.
# This is the recommended setup when Tailscale Serve or Funnel forwards an
# OAuth callback to the local Labby HTTP listener.
macos-service-install: build
    just _install-labby-bin "{{local_release_profile}}"
    scripts/install-macos-service.sh install

# Restart the installed macOS LaunchAgent without rebuilding Labby.
macos-service-restart:
    scripts/install-macos-service.sh restart

# Show the installed macOS LaunchAgent state.
macos-service-status:
    scripts/install-macos-service.sh status

# Stop and remove the per-user macOS LaunchAgent.
macos-service-uninstall:
    scripts/install-macos-service.sh uninstall

# Use the native service manager for the current OS: launchd on macOS and
# systemd on Linux. Other operating systems fail explicitly rather than
# claiming that a process is supervised when it is not.
service-install:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) just macos-service-install ;;
      Linux) just host-service-install ;;
      *) echo "error: service-install supports macOS (launchd) and Linux (systemd)" >&2; exit 1 ;;
    esac

service-restart:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) just macos-service-restart ;;
      Linux) just host-service-restart ;;
      *) echo "error: service-restart supports macOS (launchd) and Linux (systemd)" >&2; exit 1 ;;
    esac

service-status:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) just macos-service-status ;;
      Linux) just host-service-status ;;
      *) echo "error: service-status supports macOS (launchd) and Linux (systemd)" >&2; exit 1 ;;
    esac

service-uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
      Darwin) just macos-service-uninstall ;;
      Linux) just host-service-uninstall ;;
      *) echo "error: service-uninstall supports macOS (launchd) and Linux (systemd)" >&2; exit 1 ;;
    esac

# Ensure host-side runtime directories are owned by the current user before
# Docker can claim them as root during bind-mount creation.
ensure-host-dirs:
    scripts/ensure-host-dirs

# Start the explicit Docker compatibility container path for the first time (or
# after docker-compose changes).
dev-up: ensure-host-dirs
    docker compose -f docker-compose.yml up -d

# Backward-compatible alias for explicit Docker compatibility smoke.
dev: dev-container

# Backward-compatible alias for explicit Docker debug smoke.
dev-debug: dev-container-debug

# Rebuild static Labby web assets served by labby serve
web-build:
    cd apps/gateway-admin && pnpm build

# Rebuild static Labby web assets when frontend files change
web-watch:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v watchexec >/dev/null 2>&1; then
        echo "error: watchexec is required for web-watch" >&2
        echo "install: mise install watchexec" >&2
        exit 1
    fi
    echo "Building apps/gateway-admin once, then watching for changes..."
    watchexec \
      --project-origin . \
      --watch apps/gateway-admin \
      --ignore 'apps/gateway-admin/.next' \
      --ignore 'apps/gateway-admin/.next/**' \
      --ignore 'apps/gateway-admin/out' \
      --ignore 'apps/gateway-admin/out/**' \
      --ignore 'apps/gateway-admin/node_modules' \
      --ignore 'apps/gateway-admin/node_modules/**' \
      --debounce 1000ms \
      --on-busy-update queue \
      --wrap-process=none \
      'cd apps/gateway-admin && pnpm build'

# Run with args
run *ARGS:
    cargo run --all-features -- {{ARGS}}

# Run the binary-served static admin UI locally with browser auth disabled
chat-local:
    #!/usr/bin/env bash
    set -euo pipefail
    export LABBY_WEB_UI_AUTH_DISABLED=true
    export LABBY_MCP_HTTP_TOKEN="${LABBY_MCP_HTTP_TOKEN:-dev-token}"
    export LABBY_CORS_ORIGINS="${LABBY_CORS_ORIGINS:-http://node-a:3000,http://127.0.0.1:3000,http://localhost:3000}"
    export LABBY_CHAT_LOCAL_PORT="${LABBY_CHAT_LOCAL_PORT:-8766}"
    cargo run --all-features --bin labby -- serve --host 0.0.0.0 --port "${LABBY_CHAT_LOCAL_PORT}"

# Format all code
fmt:
    cargo fmt --all

# Clean build artifacts
clean:
    cargo clean

# Release (version bump + tag + push)
release *ARGS:
    cargo release {{ARGS}}

# Generate a secure MCP HTTP bearer token and write it to .env
mcp-token:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -f .env ]; then
        echo "error: .env not found — copy .env.example first" >&2
        exit 1
    fi
    token=$(openssl rand -hex 32)
    if grep -q '^LABBY_MCP_HTTP_TOKEN=' .env; then
        # macOS/BSD sed compat: write to tmp then move
        tmp=$(mktemp)
        awk -v t="$token" '/^LABBY_MCP_HTTP_TOKEN=/{print "LABBY_MCP_HTTP_TOKEN=" t; next} {print}' .env > "$tmp"
        mv "$tmp" .env
        echo "✓ rotated LABBY_MCP_HTTP_TOKEN in .env"
    else
        echo "LABBY_MCP_HTTP_TOKEN=$token" >> .env
        echo "✓ appended LABBY_MCP_HTTP_TOKEN to .env"
    fi
    echo "  $token"

# Run the prod image locally with prod-like env (LABBY_UPSTREAM_DISCOVERY_CONCURRENCY=3, no
# bind-mounted binary). Useful for testing spawn-storm safeguards and discovery timeouts that
# are masked by the dev stack's higher concurrency default (16). Starts detached, polls /health
# for up to 60s, then prints the container ID. Stop with: docker stop lab-prod-test
# See docs/OPERATIONS.md §Dev/Prod Container Drift for the full drift inventory.
prod-run: build-release
    #!/usr/bin/env bash
    set -euo pipefail
    docker stop lab-prod-test 2>/dev/null || true
    docker rm   lab-prod-test 2>/dev/null || true
    docker build -f config/Dockerfile.fast -t labby:prod-test .
    docker run -d --name lab-prod-test \
        -p 18765:8765 \
        -v "${HOME}/.labby:/home/labby/.labby" \
        -e LABBY_MCP_HTTP_HOST=0.0.0.0 \
        -e LABBY_MCP_HTTP_PORT=8765 \
        -e LABBY_UPSTREAM_DISCOVERY_CONCURRENCY=3 \
        labby:prod-test
    echo "container started — polling http://localhost:18765/health (60s timeout)..."
    deadline=$(( $(date +%s) + 60 ))
    until curl -sf http://localhost:18765/health >/dev/null 2>&1; do
        if [ "$(date +%s)" -ge "$deadline" ]; then
            echo "TIMEOUT: /health did not return 200 within 60s" >&2
            docker logs lab-prod-test >&2
            docker stop lab-prod-test
            exit 1
        fi
        sleep 2
    done
    echo "healthy — container: lab-prod-test (host port 18765)"
    echo "stop with: docker stop lab-prod-test"

# Smoke-test the lab-bg3e.3 setup wizard end-to-end against a throw-away
# LABBY_HOME. Used by CI to verify first-run detection + draft commit cycle
# without touching the operator's real ~/.labby/.
smoke-setup:
    rm -rf /tmp/lab-smoke-home
    LABBY_HOME=/tmp/lab-smoke-home cargo run --all-features -- setup --no-browser --smoke

# Validate the Labby plugin setup lifecycle against a throw-away LABBY_HOME.
validate-plugin:
    rm -rf /tmp/labby-plugin-validate
    LABBY_HOME=/tmp/labby-plugin-validate cargo run --bin labby --all-features -- setup plugin-hook --no-repair --json

# Report the currently installed Labby host-service runtime.
runtime-current:
    just host-service-status

# Compile check for the lean gateway-only slice (base services excluded).
check-gateway-slice:
    RUSTFLAGS="" cargo check -p labby --no-default-features --features gateway --all-targets

# Launch the Labby desktop palette (apps/palette-tauri) in dev mode.
palette-dev:
    cd apps/palette-tauri && pnpm tauri dev

# Build the Labby desktop palette (apps/palette-tauri) release bundle.
palette-build:
    cd apps/palette-tauri && pnpm tauri build
