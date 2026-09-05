#!/bin/sh
set -eu
if [ "${LABBY_E2E_GATE_MODE:-helper}" = browser ]; then
  registry=$LABBY_E2E_HELPER_REGISTRY
  set -- "$LABBY_E2E_BROWSER_EXECUTABLE" "$@"
else
  registry=$1
  shift
fi
if [ "${LABBY_E2E_GATE_MODE:-helper}" != browser ] && [ -n "${LABBY_E2E_ADMISSION_ID:-}" ]; then
  case "$LABBY_E2E_ADMISSION_ID" in *[!a-zA-Z0-9-]*|admission-) exit 70;; admission-*) ;; *) exit 70;; esac
  admission="$registry/$LABBY_E2E_ADMISSION_ID"
  mkdir "$admission"
else
  admission=$(mktemp -d "$registry/admission-XXXXXXXXXXXX") || exit 70
fi
admission_id=${admission##*/}
start=$(ps -o lstart= -p $$) || exit 70
printf '%s\n%s\n%s\n%s\n' "$$" "$start" "$LABBY_E2E_GROUP_TOKEN" "$admission_id" >"$admission/identity.pending"
mv "$admission/identity.pending" "$admission/identity"
entries=$(ls -A "$registry") || exit 70
if printf '%s\n' "$entries" | grep -qx closed; then exit 70; fi
group=$(ps -o pgid= -p $$) || exit 70
group=$(printf '%s' "$group" | tr -d ' ')
[ "$group" = "$$" ] || exit 70
# Once a child can exist, every ordinary shell exit must settle the owned
# group, including failure to publish its PID or completion status.
settle_owned_group() { /bin/kill -KILL -- "-$$"; }
trap settle_owned_group EXIT
trap '' TERM
(trap - TERM; exec "$@") <&0 &
child=$!
printf '%s\n' "$child" >"$admission/child.pid.pending"
mv "$admission/child.pid.pending" "$admission/child.pid"
set +e
wait "$child"
status=$?
set -e
if [ "${LABBY_E2E_GATE_MODE:-helper}" = browser ] || [ "${LABBY_E2E_GATE_MODE:-helper}" = runtime ]; then
  # Playwright and the Rust daemon harness start this guardian as the group
  # leader. Reap the group even when the real process exits before descendants.
  # dash's kill builtin rejects the negative-group/-- form; the POSIX host
  # executable accepts it consistently on supported Linux and macOS hosts.
  settle_owned_group
  exit 70
fi
printf '%s\n%s\n%s\n' "$$" "$admission_id" "$status" >"$admission/status.pending"
mv "$admission/status.pending" "$admission/status"
while :; do sleep 1; done
