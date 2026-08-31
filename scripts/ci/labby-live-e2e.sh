#!/usr/bin/env bash
set -euo pipefail
umask 077
tier="${1:-pr}"; seed="${2:-${LABBY_E2E_SEED:-1}}"
case "$tier" in pr|nightly|release|manual|collision|repeat10) ;; *) echo "invalid tier: $tier" >&2; exit 64;; esac
case "$seed" in *[!0-9]*|'') echo "seed must be numeric" >&2; exit 64;; esac
repo_root="$(cd "$(dirname "$0")/../.." && pwd -P)"
run_root="${LABBY_E2E_RUN_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/labby-live-e2e.XXXXXX")}"; case "$run_root" in /*) ;; *) exit 64;; esac
mkdir -p "$run_root/shards" "$run_root/artifacts"; chmod 700 "$run_root" "$run_root/shards" "$run_root/artifacts"
run_id="${LABBY_E2E_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$-$seed}"
export LABBY_E2E_SEED="$seed" LABBY_E2E_RUN_ID="$run_id"
primary=0; cleanup=0; evidence=0; active_pids=()
terminate_children() { for pid in "${active_pids[@]:-}"; do kill -TERM "$pid" 2>/dev/null || true; pkill -TERM -P "$pid" 2>/dev/null || true; done; for pid in "${active_pids[@]:-}"; do wait "$pid" 2>/dev/null || true; done; active_pids=(); }
finish() { status=$?; terminate_children; find "$run_root" -type l -print -quit | grep -q . && cleanup=1 || true; printf '{"primary":%s,"cleanup":%s,"evidence":%s}\n' "$primary" "$cleanup" "$evidence" >"$run_root/artifacts/status.json"; [ "$status" -eq 0 ] && [ "$primary" -eq 0 ] && [ "$cleanup" -eq 0 ] && [ "$evidence" -eq 0 ]; }
trap finish EXIT INT TERM HUP
cd "$repo_root"
if [ "$tier" = release ]; then
  : "${LABBY_RELEASE_BINARY:?release tier requires packaged LABBY_RELEASE_BINARY}"; case "$LABBY_RELEASE_BINARY" in /*) ;; *) exit 64;; esac; [ -x "$LABBY_RELEASE_BINARY" ]
  LABBY_E2E_BINARY="$(cd "$(dirname "$LABBY_RELEASE_BINARY")" && pwd -P)/$(basename "$LABBY_RELEASE_BINARY")"; export LABBY_E2E_BINARY
else
  [ "${LABBY_E2E_PREBUILT:-0}" = 1 ] || cargo build -p labby --all-features --locked
  export LABBY_E2E_BINARY="${LABBY_E2E_BINARY:-$repo_root/target/debug/labby}"
fi
case "$LABBY_E2E_BINARY" in /*) ;; *) exit 64;; esac; [ -x "$LABBY_E2E_BINARY" ]
build_id="$(shasum -a 256 "$LABBY_E2E_BINARY" | awk '{print $1}')"; binary_version="$($LABBY_E2E_BINARY --version)"; export LABBY_E2E_BUILD_IDENTITY="$build_id"
printf '{"schema_version":1,"run_id":"%s","seed":"%s","build_identity":"%s","binary":"%s","binary_version":"%s","assets":"prebuilt"}\n' "$run_id" "$seed" "$build_id" "$LABBY_E2E_BINARY" "$binary_version" >"$run_root/build-manifest.json"; chmod a-w "$run_root/build-manifest.json"
if [ "$tier" = repeat10 ]; then for repeat_seed in 1 2 3 4 5 6 7 8 9 10; do LABBY_E2E_PREBUILT=1 LABBY_E2E_BINARY="$LABBY_E2E_BINARY" "$0" pr "$repeat_seed"; done; exit 0; fi

shards=(contracts live-http-cli-api live-http-observability live-http-ipv6 live-mcp-parity live-identity-protected-restart)
if [ "$tier" = nightly ] || [ "$tier" = manual ] || [ "$tier" = release ]; then shards+=(browser-live fault-qualification); fi
if [ "$tier" = collision ]; then shards=(live-http-cli-api-a live-http-cli-api-b); fi
complete() { shard="$1"; log="$2"; if [ "$(wc -c <"$log")" -gt 1048576 ]; then tail -c 1048576 "$log" >"$log.bounded"; mv "$log.bounded" "$log"; fi; hash="$(shasum -a 256 "$log" | awk '{print $1}')"; printf '{"schema_version":1,"run_id":"%s","seed":"%s","build_identity":"%s","shard":"%s","status":"passed","sha256":"%s"}\n' "$run_id" "$seed" "$build_id" "$shard" "$hash" >"$run_root/shards/$shard.json"; }
run_shard() {
  shard="$1"; log="$run_root/$shard.log"
  case "$shard" in
    contracts) cargo test -p labby --all-features --test action_matrix_completeness --locked >"$log" 2>&1;;
    live-http-cli-api*) cargo test -p labby --all-features --test live_http_routes --test live_cli_actions --test live_api_actions --locked -- --test-threads=1 >"$log" 2>&1;;
    live-http-observability) cargo test -p labby --all-features --test live_http_observability --locked -- --test-threads=1 >"$log" 2>&1;;
    live-http-ipv6) cargo test -p labby --all-features --test live_http_ipv6 --locked -- --test-threads=1 >"$log" 2>&1;;
    live-mcp-parity) cargo test -p labby --all-features --test live_mcp_actions --test live_surface_parity --locked -- --test-threads=1 >"$log" 2>&1;;
    live-identity-protected-restart) cargo test -p labby --all-features --test live_identity_bootstrap --test live_protected_routes --test live_restart_persistence --locked -- --test-threads=1 >"$log" 2>&1;;
    browser-live) node_bin="$(command -v node)"; case "$node_bin" in */mise/shims/*) node_bin="$(mise which node)";; esac; PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-/home/runner/.cache/ms-playwright}" LABBY_NODE_BIN="$node_bin" LABBY_LIVE_BROWSER_RUN=1 LABBY_LIVE_BROWSER_ASSETS_DIR="${LABBY_LIVE_BROWSER_ASSETS_DIR:-$repo_root/apps/gateway-admin/out}" cargo test -p labby --all-features --test live_browser_supervisor --locked -- --test-threads=1 >"$log" 2>&1;;
    fault-qualification) LABBY_E2E_FAULT_REPORT="$run_root/artifacts/fault-qualification.json" cargo test -p labby --all-features --test e2e_fault_qualification --locked -- --test-threads=1 >"$log" 2>&1;;
  esac && complete "$shard" "$log"
}
if [ "$tier" = collision ]; then for shard in "${shards[@]}"; do run_shard "$shard" & active_pids+=("$!"); done; for pid in "${active_pids[@]}"; do wait "$pid" || primary=1; done; active_pids=(); else for shard in "${shards[@]}"; do run_shard "$shard" || { primary=1; tail -c 12000 "$run_root/$shard.log" >&2 || true; exit 1; }; done; fi
[ "$primary" -eq 0 ] || exit 1
find "$run_root" -type l -print -quit | grep -q . && cleanup=1 || true
evidence_canary="LABBY_E2E_RETAINED_SECRET_${run_id}"; grep -R -F -l "$evidence_canary" "$run_root/artifacts" "$run_root"/*.log 2>/dev/null | grep -q . && evidence=1 || true
printf '{"schema_version":1,"owned_children_absent":%s,"symlinks_absent":%s,"secret_canary_absent":%s}\n' "$([ "$cleanup" -eq 0 ] && echo true || echo false)" "$([ "$cleanup" -eq 0 ] && echo true || echo false)" "$([ "$evidence" -eq 0 ] && echo true || echo false)" >"$run_root/artifacts/residual-audit.json"
[ "$cleanup" -eq 0 ] && [ "$evidence" -eq 0 ] || exit 1
if [ "$tier" = collision ]; then
  printf '{"schema_version":1,"run_id":"%s","seed":"%s","build_identity":"%s","status":"passed","shards":["live-http-cli-api-a","live-http-cli-api-b"]}\n' "$run_id" "$seed" "$build_id" >"$run_root/artifacts/collision.json"
  echo "live E2E collision passed: $run_root/artifacts/collision.json"
  exit 0
fi
LABBY_E2E_DECLARED_SHARDS="$(IFS=,; echo "${shards[*]}")"; export LABBY_E2E_DECLARED_SHARDS
export LABBY_E2E_SHARD_DIR="$run_root/shards" LABBY_E2E_REPORT="$run_root/artifacts/coverage.json" LABBY_E2E_CLEANUP_STATUS=passed LABBY_E2E_EVIDENCE_STATUS=passed
cargo test -p labby --all-features --test e2e_coverage_report --locked -- --exact
shasum -a 256 "$run_root/artifacts/coverage.json" >"$run_root/artifacts/coverage.json.sha256"
echo "live E2E $tier passed: $run_root/artifacts/coverage.json"
