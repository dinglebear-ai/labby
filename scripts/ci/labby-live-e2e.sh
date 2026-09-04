#!/usr/bin/env bash
set -euo pipefail
umask 077
tier="${1:-pr}"; seed="${2:-${LABBY_E2E_SEED:-1}}"
case "$tier" in pr|nightly|release|manual|collision|repeat10) ;; *) echo "invalid tier: $tier" >&2; exit 64;; esac
case "$seed" in *[!0-9]*|'') echo "seed must be numeric" >&2; exit 64;; esac
repo_root="$(cd "$(dirname "$0")/../.." && pwd -P)"
if [ -n "${LABBY_E2E_RUN_ROOT:-}" ]; then
  run_root="$LABBY_E2E_RUN_ROOT"; case "$run_root" in /*) ;; *) exit 64;; esac
  mkdir "$run_root"
else
  run_root="$(mktemp -d "${TMPDIR:-/tmp}/labby-live-e2e.XXXXXX")"
fi
mkdir "$run_root/shards" "$run_root/cases" "$run_root/artifacts"; chmod 700 "$run_root" "$run_root/shards" "$run_root/cases" "$run_root/artifacts"
helper_registry="$run_root/helper-groups"
mkdir "$helper_registry"; chmod 700 "$helper_registry"
export LABBY_E2E_HELPER_REGISTRY="$helper_registry"
run_id="${LABBY_E2E_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$-$seed}"
export LABBY_E2E_SEED="$seed" LABBY_E2E_RUN_ID="$run_id"
export LABBY_E2E_CASE_DIR="$run_root/cases"
secret_registry="$run_root/scan-secrets.txt"
evidence_canary="LABBY_E2E_RETAINED_SECRET_${run_id}_$(shasum -a 256 <<<"$run_id:$seed:$$" | awk '{print $1}')"
printf '%s\n' "$evidence_canary" >"$secret_registry"; chmod 600 "$secret_registry"
export LABBY_E2E_RETAINED_SECRET="$evidence_canary"
primary=0; cleanup=0; evidence=0; active_pids=(); owned_groups=(); group_seq=0
touch "$run_root/process-groups.tsv" "$run_root/process-group-members.tsv"
group_alive() { kill -0 -- "-$1" 2>/dev/null; }
register_group() { printf '%s\t%s\t%s\n' "$1" "$(ps -o lstart= -p "$1" | sed 's/^ *//;s/ *$//')" "$2" >>"$run_root/process-groups.tsv"; }
refresh_group_members() {
  local group="$1" inventory member member_group member_start
  inventory="$(if [ "${LABBY_E2E_MEMBER_INVENTORY_FAILURE_SELFTEST:-0}" = 1 ]; then exit 1; else ps -axo pid=,pgid=,lstart=; fi)" || { cleanup=1; return 70; }
  while read -r member member_group member_start; do
    [ "$member_group" = "$group" ] || continue
    [ -n "$member_start" ] || { cleanup=1; return 70; }
    printf '%s\t%s\t%s\n' "$group" "$member" "$member_start" >>"$run_root/process-group-members.tsv"
  done <<<"$inventory"
}
group_identity_matches() {
  expected="$(awk -F '\t' -v group="$1" '$1 == group { print $2; exit }' "$run_root/process-groups.tsv" 2>/dev/null)"
  token="$(awk -F '\t' -v group="$1" '$1 == group { print $3; exit }' "$run_root/process-groups.tsv" 2>/dev/null)"
  current="$(ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//;s/ *$//')"
  [ -n "$expected" ] && [ "$current" = "$expected" ] && return 0
  while IFS=$'\t' read -r recorded_group member member_start; do
    [ "$recorded_group" = "$1" ] || continue
    current="$(ps -o lstart= -p "$member" 2>/dev/null | sed 's/^ *//;s/ *$//')"
    [ -n "$member_start" ] && [ "$current" = "$member_start" ] && return 0
  done <"$run_root/process-group-members.tsv" 2>/dev/null || true
  [ -n "$token" ] || return 1
  for member in $(ps -axo pid=,pgid= | awk -v group="$1" '$2 == group { print $1 }'); do
    ps eww -p "$member" -o command= 2>/dev/null | tr ' ' '\n' | grep -Fqx "LABBY_E2E_GROUP_TOKEN=$token" && return 0
  done
  return 1
}
group_has_listener() {
  group="$1"
  for pid in $(ps -axo pid=,pgid= | awk -v group="$group" '$2 == group { print $1 }'); do
    lsof -nP -a -p "$pid" -iTCP -sTCP:LISTEN 2>/dev/null | grep -q . && return 0
  done
  return 1
}
adopt_cleanup_helpers() {
  local helper pid member token inventory recorded_start current_start
  inventory="$(if [ "${LABBY_E2E_INVENTORY_FAILURE_SELFTEST:-0}" = 1 ]; then exit 1; else ps -axo pid=,pgid=; fi)" || { cleanup=1; return 70; }
  for helper in "$helper_registry"/[0-9]*; do
    [ -d "$helper" ] && [ ! -L "$helper" ] || continue
    pid="${helper##*/}"
    case "$pid" in *[!0-9]*|'') cleanup=1; continue;; esac
    member="$(printf '%s\n' "$inventory" | awk -v group="$pid" '$2 == group { print $1; exit }')"
    [ -n "$member" ] || continue
    # The gate keeps its group leader alive until group reap; no platform-
    # dependent environment introspection is needed to verify ownership.
    [ -f "$helper/identity" ] && [ ! -L "$helper/identity" ] || { cleanup=1; continue; }
    recorded_start="$(sed -n '1p' "$helper/identity" | sed 's/^ *//;s/ *$//')"
    token="$(sed -n '2p' "$helper/identity")"
    current_start="$(ps -o lstart= -p "$pid" 2>/dev/null)" || { cleanup=1; continue; }
    current_start="$(printf '%s' "$current_start" | sed 's/^ *//;s/ *$//')"
    [ -n "$recorded_start" ] && [ "$recorded_start" = "$current_start" ] || { cleanup=1; continue; }
    if [ -z "$token" ] || ! awk -F '\t' -v token="$token" '$3 == token { found=1 } END { exit !found }' "$run_root/process-groups.tsv"; then
      cleanup=1
      continue
    fi
    owned_groups+=("$pid")
    register_group "$pid" "$token"
  done
}
terminate_children() {
  set +m
  # Closing admission precedes the registry scan: later helper gates must exit
  # before exec, and every previously admitted helper already has a directory.
  mkdir "$helper_registry/closed" 2>/dev/null || [ -d "$helper_registry/closed" ] || { cleanup=1; return 70; }
  adopt_cleanup_helpers || cleanup=1
  termination_deadline="${termination_deadline:-$((SECONDS + 5))}"
  for group in "${owned_groups[@]:-}"; do
    refresh_group_members "$group" || cleanup=1
    if ! group_identity_matches "$group" || ! kill -TERM -- "-$group" 2>/dev/null; then
      if group_alive "$group"; then cleanup=1; fi
    fi
  done
  deadline=$((SECONDS + 3))
  [ "$deadline" -le "$termination_deadline" ] || deadline="$termination_deadline"
  while [ "$SECONDS" -lt "$deadline" ]; do
    alive=0; for group in "${owned_groups[@]:-}"; do group_alive "$group" && alive=1; done
    [ "$alive" -eq 0 ] && break
    sleep 0.05
  done
  for group in "${owned_groups[@]:-}"; do
    if group_identity_matches "$group" && group_alive "$group"; then
      kill -KILL -- "-$group" 2>/dev/null || { cleanup=1; group_alive "$group" && return 70 || true; }
    fi
  done
  reap_deadline=$((SECONDS + 2))
  [ "$reap_deadline" -le "$termination_deadline" ] || reap_deadline="$termination_deadline"
  while [ "$SECONDS" -lt "$reap_deadline" ]; do
    alive=0; for group in "${owned_groups[@]:-}"; do group_alive "$group" && alive=1; done
    [ "$alive" -eq 0 ] && break
    sleep 0.05
  done
  for group in "${owned_groups[@]:-}"; do group_alive "$group" && { cleanup=1; return 70; } || true; done
  for pid in "${active_pids[@]:-}"; do wait "$pid" 2>/dev/null || true; done
  active_pids=()
  owned_groups=()
}
finish() {
  status=$?
  trap - EXIT
  [ "$status" -eq 0 ] || primary=1
  terminate_children || cleanup=1
  find "$run_root" -type l -print -quit | grep -q . && cleanup=1 || true
  printf '{"primary":%s,"cleanup":%s,"evidence":%s}\n' "$primary" "$cleanup" "$evidence" >"$run_root/artifacts/status.json"
  if [ "$status" -eq 0 ] && { [ "$primary" -ne 0 ] || [ "$cleanup" -ne 0 ] || [ "$evidence" -ne 0 ]; }; then status=1; fi
  exit "$status"
}
cancel() { trap - INT TERM HUP; exit "$1"; }
trap finish EXIT
trap 'cancel 130' INT
trap 'cancel 143' TERM
trap 'cancel 129' HUP
if [ "${LABBY_E2E_SIGNAL_SELFTEST:-0}" = 1 ]; then
  set -m
  group_seq=$((group_seq + 1)); group_token="$run_id-group-$group_seq"
  LABBY_E2E_GROUP_TOKEN="$group_token" bash -c 'trap "" TERM; while :; do sleep 1; done' &
  pid="$!"; active_pids+=("$pid"); owned_groups+=("$pid")
  register_group "$pid" "$group_token"
  printf '%s\n' "$pid" >"$run_root/signal-selftest.ready"
  wait "$pid"
fi
if [ "${LABBY_E2E_EXIT_FAILURE_SELFTEST:-0}" = 1 ]; then cleanup=1; exit 0; fi
if [ "${LABBY_E2E_INVENTORY_FAILURE_SELFTEST:-0}" = 1 ] || [ "${LABBY_E2E_MEMBER_INVENTORY_FAILURE_SELFTEST:-0}" = 1 ]; then exit 0; fi
if [ "${LABBY_E2E_LISTENER_SELFTEST:-0}" = 1 ]; then
  set -m
  group_seq=$((group_seq + 1)); group_token="$run_id-group-$group_seq"
  LABBY_E2E_GROUP_TOKEN="$group_token" python3 -c 'import socket,time; s=socket.socket(); s.bind(("127.0.0.1",0)); s.listen(); time.sleep(30)' &
  pid="$!"; active_pids+=("$pid"); owned_groups+=("$pid")
  register_group "$pid" "$group_token"
  deadline=$((SECONDS + 3)); while ! group_has_listener "$pid" && [ "$SECONDS" -lt "$deadline" ]; do sleep 0.05; done
  detected=false; group_has_listener "$pid" && detected=true
  printf '{"schema_version":1,"owned_listener_detected":%s}\n' "$detected" >"$run_root/artifacts/listener-selftest.json"
  [ "$detected" = true ] || evidence=1
  exit 0
fi
if [ "${LABBY_E2E_CLEANUP_SELFTEST:-0}" = 1 ]; then
  set -m
  group_seq=$((group_seq + 1)); group_token="$run_id-group-$group_seq"
  LABBY_E2E_GROUP_TOKEN="$group_token" bash -c 'trap "" TERM; (trap "" TERM; while :; do sleep 1; done) & wait' &
  pid="$!"
  active_pids+=("$pid")
  owned_groups+=("$pid")
  register_group "$pid" "$group_token"
  sleep 0.1
  terminate_children
  group_alive "$pid" && cleanup=1 || true
  printf '{"schema_version":1,"owned_children_absent":%s}\n' "$([ "$cleanup" -eq 0 ] && echo true || echo false)" >"$run_root/artifacts/cleanup-selftest.json"
  [ "$cleanup" -eq 0 ]
  exit 0
fi
if [ "${LABBY_E2E_RETAINED_GROUP_SELFTEST:-0}" = 1 ]; then
  set -m
  group_seq=$((group_seq + 1)); group_token="$run_id-group-$group_seq"
  LABBY_E2E_GROUP_TOKEN="$group_token" bash -c '(trap "" TERM; while :; do sleep 1; done) & printf "%s\n" "$!" >"$LABBY_E2E_RUN_ROOT/retained-child.pid"' &
  pid="$!"; active_pids+=("$pid"); owned_groups+=("$pid"); register_group "$pid" "$group_token"
  wait "$pid"; active_pids=(); refresh_group_members "$pid"
  deadline=$((SECONDS + 3)); while [ ! -s "$run_root/retained-child.pid" ] && [ "$SECONDS" -lt "$deadline" ]; do sleep 0.05; done
  terminate_children
  group_alive "$pid" && cleanup=1 || true
  printf '{"schema_version":1,"retained_group_absent":%s}\n' "$([ "$cleanup" -eq 0 ] && echo true || echo false)" >"$run_root/artifacts/retained-group-selftest.json"
  [ "$cleanup" -eq 0 ]
  exit 0
fi
if [ "${LABBY_E2E_SECRET_SCAN_SELFTEST:-0}" = 1 ]; then
  mkdir "$run_root/artifacts/nested"
  printf 'prefix %s suffix\n' "$evidence_canary" >"$run_root/artifacts/nested/retained.txt"
  if grep -R -a -F -f "$secret_registry" --exclude="$(basename "$secret_registry")" "$run_root" 2>/dev/null | grep -q .; then
    rm -f "$run_root/artifacts/nested/retained.txt" "$secret_registry"
    printf '{"schema_version":1,"retained_secret_detected":true}\n' >"$run_root/artifacts/secret-scan-selftest.json"
    exit 0
  fi
  evidence=1
  exit 1
fi
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
if [ "$tier" = repeat10 ]; then
  mkdir "$run_root/repeats"
  summaries=""
  for repeat_seed in 1 2 3 4 5 6 7 8 9 10; do
    child_root="$run_root/repeats/seed-$repeat_seed"
    LABBY_E2E_RUN_ROOT="$child_root" LABBY_E2E_PREBUILT=1 LABBY_E2E_BINARY="$LABBY_E2E_BINARY" "$0" pr "$repeat_seed"
    coverage_sha="$(awk '{print $1}' "$child_root/artifacts/coverage.json.sha256")"
    [ -z "$summaries" ] || summaries="$summaries,"
    summaries="$summaries{\"seed\":$repeat_seed,\"coverage_sha256\":\"$coverage_sha\"}"
  done
  printf '{"schema_version":1,"run_id":"%s","build_identity":"%s","repeats":[%s]}\n' "$run_id" "$build_id" "$summaries" >"$run_root/artifacts/repeat10.json"
  # Child runs have each completed their own residual audit. Audit the bounded
  # aggregate tree after the parent evidence has been retained as well.
  find "$run_root" -type l -print -quit | grep -q . && cleanup=1 || true
  find "$run_root" -type f -size +33554432c -print -quit | grep -q . && evidence=1 || true
  secret_canary_absent=true; grep -R -a -F -f "$secret_registry" --exclude="$(basename "$secret_registry")" "$run_root" 2>/dev/null | grep -q . && { secret_canary_absent=false; evidence=1; } || true
  rm -f "$secret_registry"
  printf '{"schema_version":1,"owned_children_absent":true,"owned_listeners_absent":true,"symlinks_absent":%s,"bounded_files":%s,"secret_canary_absent":%s}\n' "$([ "$cleanup" -eq 0 ] && echo true || echo false)" "$([ "$evidence" -eq 0 ] && echo true || echo false)" "$secret_canary_absent" >"$run_root/artifacts/residual-audit.json"
  printf '{"primary":%s,"cleanup":%s,"evidence":%s}\n' "$primary" "$cleanup" "$evidence" >"$run_root/artifacts/status.json"
  [ "$cleanup" -eq 0 ] && [ "$evidence" -eq 0 ]
  exit 0
fi

shards=(contracts live-http-cli-api live-http-observability live-http-ipv6 live-mcp-parity live-identity-protected-restart)
if [ "$tier" = nightly ] || [ "$tier" = manual ] || [ "$tier" = release ]; then shards+=(browser-live fault-qualification); fi
if [ "$tier" = collision ]; then shards=(live-http-cli-api-a live-http-cli-api-b); fi
complete() { shard="$1"; log="$2"; if [ "$(wc -c <"$log")" -gt 1048576 ]; then tail -c 1048576 "$log" >"$log.bounded"; mv "$log.bounded" "$log"; fi; hash="$(shasum -a 256 "$log" | awk '{print $1}')"; printf '{"schema_version":1,"run_id":"%s","seed":"%s","build_identity":"%s","shard":"%s","status":"passed","sha256":"%s"}\n' "$run_id" "$seed" "$build_id" "$shard" "$hash" >"$run_root/shards/$shard.json"; }
run_shard() {
  # The parent creates this background shard's process group with monitor mode.
  # Inside it, foreground commands must inherit that group rather than creating
  # additional unregistered job-control groups.
  set +m
  shard="$1"; log="$run_root/$shard.log"
  case "$shard" in
    contracts) cargo test -p labby --all-features --test action_matrix_completeness --locked >"$log" 2>&1;;
    live-http-cli-api*) cargo test -p labby --all-features --test live_http_routes --test live_cli_actions --test live_api_actions --locked -- --test-threads=1 >"$log" 2>&1;;
    live-http-observability) cargo test -p labby --all-features --test live_http_observability --locked -- --test-threads=1 >"$log" 2>&1;;
    live-http-ipv6) cargo test -p labby --all-features --test live_http_ipv6 --locked -- --test-threads=1 >"$log" 2>&1;;
    live-mcp-parity) cargo test -p labby --all-features --test live_mcp_actions --test live_surface_parity --locked -- --test-threads=1 >"$log" 2>&1;;
    live-identity-protected-restart) cargo test -p labby --all-features --test live_identity_bootstrap --test live_protected_routes --test live_restart_persistence --locked -- --test-threads=1 >"$log" 2>&1;;
    browser-live) node_bin="$(command -v node)"; case "$node_bin" in */mise/shims/*) node_bin="$(mise which node)";; esac; PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-/home/runner/.cache/ms-playwright}" LABBY_NODE_BIN="$node_bin" LABBY_LIVE_BROWSER_RUN=1 LABBY_LIVE_BROWSER_NIGHTLY="$([ "$tier" = nightly ] && echo true || echo false)" LABBY_LIVE_BROWSER_ASSETS_DIR="${LABBY_LIVE_BROWSER_ASSETS_DIR:-$repo_root/apps/gateway-admin/out}" cargo test -p labby --all-features --test live_browser_supervisor --locked -- --test-threads=1 >"$log" 2>&1;;
    fault-qualification) LABBY_E2E_FAULT_REPORT="$run_root/artifacts/fault-qualification.json" cargo test -p labby --all-features --test e2e_fault_qualification --locked -- --test-threads=1 >"$log" 2>&1;;
    wedged-cleanup-selftest) bash -c 'trap "" TERM; sleep 6; touch -- "$LABBY_E2E_WEDGED_MARKER"; while :; do sleep 1; done' >"$log" 2>&1;;
    escaped-cleanup-selftest) "$LABBY_E2E_HELPER_TEST_BINARY" "$LABBY_E2E_HELPER_TEST_FILTER" --exact --ignored --nocapture >"$log" 2>&1;;
    escaped-browser-selftest) "$LABBY_NODE_BIN" --experimental-strip-types "$repo_root/apps/gateway-admin/lib/browser/noncooperative-browser-parent.fixture.ts" >"$log" 2>&1;;
  esac && complete "$shard" "$log"
}
start_owned_shard() {
  shard="$1"; group_seq=$((group_seq + 1)); group_token="$run_id-group-$group_seq"
  LABBY_E2E_GROUP_TOKEN="$group_token" run_shard "$shard" & last_pid="$!"
  active_pids+=("$last_pid"); owned_groups+=("$last_pid"); register_group "$last_pid" "$group_token"
}
wait_owned_shard() {
  pid="$1"; shard="$2"; limit="${LABBY_E2E_SHARD_TIMEOUT_SECONDS:-900}"
  case "$limit" in *[!0-9]*|'') return 64;; esac
  shard_deadline=$((SECONDS + limit))
  [ "$shard_deadline" -le "$run_deadline" ] || shard_deadline="$run_deadline"
  while group_alive "$pid" && [ "$SECONDS" -lt "$shard_deadline" ]; do sleep 0.05; done
  if group_alive "$pid"; then
    primary=1
    terminate_children || return 70
    return 124
  fi
  wait "$pid" || return $?
}
if [ "${LABBY_E2E_WEDGED_SHARD_SELFTEST:-0}" = 1 ]; then
  shards=(wedged-cleanup-selftest)
  export LABBY_E2E_WEDGED_MARKER="$run_root/post-deadline-mutation"
fi
if [ "${LABBY_E2E_ESCAPED_HELPER_SELFTEST:-0}" = 1 ]; then
  shards=(escaped-cleanup-selftest)
  export LABBY_E2E_WEDGED_MARKER="$run_root/post-deadline-mutation"
fi
if [ "${LABBY_E2E_ESCAPED_BROWSER_SELFTEST:-0}" = 1 ]; then
  shards=(escaped-browser-selftest)
  export LABBY_E2E_BROWSER_FIXTURE_MARKER="$run_root/detached-browser.json"
fi
run_limit="${LABBY_E2E_RUN_TIMEOUT_SECONDS:-7200}"
case "$run_limit" in *[!0-9]*|'') exit 64;; esac
run_deadline=$((SECONDS + run_limit))
set -m
if [ "$tier" = collision ]; then
  shard_pids=(); for shard in "${shards[@]}"; do start_owned_shard "$shard"; shard_pids+=("$last_pid:$shard"); done
  for entry in "${shard_pids[@]}"; do pid="${entry%%:*}"; shard="${entry#*:}"; wait_owned_shard "$pid" "$shard" || primary=1; refresh_group_members "$pid"; done
else
  for shard in "${shards[@]}"; do start_owned_shard "$shard"; wait_owned_shard "$last_pid" "$shard" || { primary=1; tail -c 12000 "$run_root/$shard.log" >&2 || true; break; }; done
fi
active_pids=(); set +m
[ "$primary" -eq 0 ] || exit 1
# Audit before producing any aggregate artifact that claims this run passed.
symlinks_absent=true; find "$run_root" -type l -print -quit | grep -q . && { symlinks_absent=false; cleanup=1; } || true
owned_children_absent=true; for group in "${owned_groups[@]:-}"; do group_alive "$group" && { owned_children_absent=false; cleanup=1; }; done
owned_listeners_absent=true; for group in "${owned_groups[@]:-}"; do group_has_listener "$group" && { owned_listeners_absent=false; cleanup=1; }; done
oversized_absent=true; find "$run_root" -type f -size +33554432c -print -quit | grep -q . && { oversized_absent=false; evidence=1; } || true
secret_canary_absent=true; grep -R -a -F -f "$secret_registry" --exclude="$(basename "$secret_registry")" "$run_root" 2>/dev/null | grep -q . && { secret_canary_absent=false; evidence=1; } || true
rm -f "$secret_registry"
printf '{"schema_version":1,"owned_children_absent":%s,"owned_listeners_absent":%s,"symlinks_absent":%s,"bounded_files":%s,"secret_canary_absent":%s}\n' "$owned_children_absent" "$owned_listeners_absent" "$symlinks_absent" "$oversized_absent" "$secret_canary_absent" >"$run_root/artifacts/residual-audit.json"
[ "$cleanup" -eq 0 ] && [ "$evidence" -eq 0 ] || exit 1
if [ "$tier" = collision ]; then
  printf '{"schema_version":1,"run_id":"%s","seed":"%s","build_identity":"%s","status":"passed","shards":["live-http-cli-api-a","live-http-cli-api-b"]}\n' "$run_id" "$seed" "$build_id" >"$run_root/artifacts/collision.json"
else
  LABBY_E2E_DECLARED_SHARDS="$(IFS=,; echo "${shards[*]}")"; export LABBY_E2E_DECLARED_SHARDS
  export LABBY_E2E_SHARD_DIR="$run_root/shards" LABBY_E2E_REPORT="$run_root/artifacts/coverage.json" LABBY_E2E_CLEANUP_STATUS=passed LABBY_E2E_EVIDENCE_STATUS=passed
  cargo test -p labby --all-features --test e2e_coverage_report --locked -- --exact
  shasum -a 256 "$run_root/artifacts/coverage.json" >"$run_root/artifacts/coverage.json.sha256"
fi
printf '{"primary":%s,"cleanup":%s,"evidence":%s}\n' "$primary" "$cleanup" "$evidence" >"$run_root/artifacts/status.json"
if [ "$tier" = collision ]; then echo "live E2E collision passed: $run_root/artifacts/collision.json"; else echo "live E2E $tier passed: $run_root/artifacts/coverage.json"; fi
