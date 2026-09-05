#!/usr/bin/env bash
set -euo pipefail

base=${LABBY_QUALIFY_BASE_URL:?HTTPS operator route required}
token=${LABBY_QUALIFY_TOKEN:?operator bearer token required}
ca=${LABBY_QUALIFY_CA_CERT:?trusted route CA certificate required}
root=${LABBY_QUALIFY_RESOURCE_ROOT:?expected protected-resource root required}
service=${LABBY_QUALIFY_UPSTREAM_SERVICE:?representative upstream service required}
action=${LABBY_QUALIFY_UPSTREAM_ACTION:?representative upstream action required}
params=${LABBY_QUALIFY_UPSTREAM_PARAMS:-'{}'}
restart=${LABBY_QUALIFY_RESTART:?restart probe executable required}
backup=${LABBY_QUALIFY_BACKUP_OBSERVER:?backup observer executable required}
observer_timeout=${LABBY_QUALIFY_OBSERVER_TIMEOUT_SECONDS:-30}
[[ $observer_timeout =~ ^[0-9]+([.][0-9]+)?$ ]] || { echo "observer timeout must be a positive number" >&2; exit 64; }
[[ $base == https://* ]] || { echo "operator route must use HTTPS" >&2; exit 64; }
[[ -x $restart && -x $backup ]] || { echo "restart and backup probes must be executable files" >&2; exit 64; }

curl_args=(--fail --silent --show-error --connect-timeout 10 --max-time 30 --cacert "$ca" --proto '=https' --tlsv1.2)
curl "${curl_args[@]}" "$base/ready" >/dev/null
metadata=$(curl "${curl_args[@]}" "$base/.well-known/oauth-protected-resource")
python3 - "$root" "$metadata" <<'PY'
import json, sys
expected, raw = sys.argv[1:]
actual = json.loads(raw).get("resource")
if actual != expected:
    raise SystemExit(f"selected protected-resource root differs: {actual!r} != {expected!r}")
PY
status=$(curl --silent --show-error --connect-timeout 10 --max-time 30 --cacert "$ca" --proto '=https' --tlsv1.2 \
  --output /dev/null --write-out '%{http_code}' -H 'Content-Type: application/json' \
  --data "{\"action\":\"$action\",\"params\":$params}" "$base/v1/$service")
[[ $status == 401 || $status == 403 ]] || { echo "unauthenticated operator action was not rejected" >&2; exit 1; }
call() {
  curl "${curl_args[@]}" -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
    --data "$2" "$base/v1/$1"
}
run_observer() {
  python3 - "$observer_timeout" "$@" <<'PY'
import os, signal, subprocess, sys

timeout = float(sys.argv[1])
p = subprocess.Popen(sys.argv[2:], text=True, stdout=subprocess.PIPE,
                     start_new_session=True)
try:
    stdout, _ = p.communicate(timeout=timeout)
except subprocess.TimeoutExpired:
    os.killpg(os.getpgid(p.pid), signal.SIGKILL)
    p.communicate()
    raise SystemExit(
        f"operator observer timed out after {timeout:g} seconds: {sys.argv[2:]!r}"
    )
if p.returncode != 0:
    raise SystemExit(p.returncode)
sys.stdout.write(stdout)
PY
}
call "$service" "{\"action\":\"$action\",\"params\":$params}" >/dev/null
name="operator-qualification-${GITHUB_RUN_ID:-local}"
before=$(run_observer "$backup" latest)
[[ -n $before ]] || { echo "backup observer returned no known-good identity" >&2; exit 1; }
call snippets "{\"action\":\"snippets.create\",\"params\":{\"name\":\"$name\",\"body\":\"async () => ({ value: \\\"durable\\\" })\",\"description\":\"operator qualification\",\"force\":true}}" >/dev/null
run_observer "$backup" create
after=$(run_observer "$backup" latest)
[[ -n $after && $after != "$before" ]] || { echo "no new observed backup after durable operator work" >&2; exit 1; }
run_observer "$backup" contains "$after" "$name"
run_observer "$restart"
curl "${curl_args[@]}" "$base/ready" >/dev/null
call snippets "{\"action\":\"snippets.get\",\"params\":{\"name\":\"$name\"}}" | grep -F durable >/dev/null
call snippets "{\"action\":\"snippets.remove\",\"params\":{\"name\":\"$name\"}}" >/dev/null
run_observer "$backup" restore "$after"
run_observer "$restart"
curl "${curl_args[@]}" "$base/ready" >/dev/null
call snippets "{\"action\":\"snippets.get\",\"params\":{\"name\":\"$name\"}}" | grep -F durable >/dev/null
printf '{"route":"%s","resource":"%s","upstream":"%s/%s","backup_before":"%s","backup_after":"%s","status":"qualified"}\n' \
  "$base" "$root" "$service" "$action" "$before" "$after"
