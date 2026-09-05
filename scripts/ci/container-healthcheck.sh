#!/bin/sh
set -eu
state_dir=${LABBY_HEALTH_STATE_DIR:-/home/labby/.local/state/labby}
state=$state_dir/health-failures
log=$state_dir/health-recovery.log
log_max_bytes=${LABBY_HEALTH_LOG_MAX_BYTES:-65536}
log_keep_bytes=${LABBY_HEALTH_LOG_KEEP_BYTES:-32768}
for value in "$log_max_bytes" "$log_keep_bytes"; do
  case "$value" in
    ''|*[!0-9]*) echo "health log limits must be positive integers" >&2; exit 64 ;;
  esac
  test "$value" -gt 0 || { echo "health log limits must be positive integers" >&2; exit 64; }
done
if test "$log_keep_bytes" -gt "$log_max_bytes"; then
  log_keep_bytes=$log_max_bytes
fi
mkdir -p "$state_dir"
append_log() {
  printf '%s\n' "$1" >>"$log"
  size=$(wc -c <"$log")
  if test "$size" -gt "$log_max_bytes"; then
    tail -c "$log_keep_bytes" "$log" >"$log.tmp"
    mv "$log.tmp" "$log"
  fi
}
# Leave enough headroom inside Compose's five-second healthcheck deadline to
# persist the failure and request recovery even when the TCP peer accepts but
# never responds.
if curl -fsS --connect-timeout 1 --max-time 2 http://127.0.0.1:8765/health >/dev/null; then
  test ! -f "$state" || append_log "$(date -u +%FT%TZ) recovered"
  rm -f "$state"
  exit 0
fi
count=0
test ! -f "$state" || count=$(cat "$state")
case "$count" in *[!0-9]*) count=0 ;; esac
count=$((count + 1))
printf '%s\n' "$count" >"$state"
append_log "$(date -u +%FT%TZ) health_failure count=$count"
if test "$count" -ge 9; then
  echo "labby recovery exhausted after $count failed probes" >&2
  exit 1
fi
if test "$count" -ge 3; then
  case "$count" in 3) delay=1;; 4) delay=2;; *) delay=3;; esac
  if test "${LABBY_HEALTH_TEST_MODE:-0}" != 1; then sleep "$delay"; fi
  append_log "$(date -u +%FT%TZ) restart_requested delay=$delay"
  if test "${LABBY_HEALTH_TEST_MODE:-0}" != 1; then kill -TERM 1; fi
fi
exit 1
