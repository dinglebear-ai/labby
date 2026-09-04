#!/bin/sh
set -eu
if [ "${LABBY_E2E_GATE_MODE:-helper}" = browser ]; then
  registry=$LABBY_E2E_HELPER_REGISTRY
  set -- "$LABBY_E2E_BROWSER_EXECUTABLE" "$@"
else
  registry=$1
  shift
fi
mkdir "$registry/$$"
start=$(ps -o lstart= -p $$) || exit 70
printf '%s\n%s\n' "$start" "$LABBY_E2E_GROUP_TOKEN" >"$registry/$$/identity"
entries=$(ls -A "$registry") || exit 70
if printf '%s\n' "$entries" | grep -qx closed; then exit 70; fi
trap '' TERM
(trap - TERM; exec "$@") <&0 &
child=$!
set +e
wait "$child"
status=$?
set -e
if [ "${LABBY_E2E_GATE_MODE:-helper}" = browser ]; then
  # Playwright starts this guardian as the detached process-group leader.
  # Reap the entire group even when Chromium exits before its descendants.
  group=$(ps -o pgid= -p $$) || exit 70
  group=$(printf '%s' "$group" | tr -d ' ')
  [ "$group" = "$$" ] || exit 70
  kill -KILL -- "-$$"
  exit 70
fi
printf '%s\n' "$status" >"$registry/$$/status.pending"
mv "$registry/$$/status.pending" "$registry/$$/status"
while :; do sleep 1; done
