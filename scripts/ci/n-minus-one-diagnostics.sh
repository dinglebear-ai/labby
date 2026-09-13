#!/usr/bin/env bash
# Print what a failed N-1 leg left behind: service logs, the systemd journal,
# and Incus state. Never fails itself, and never prints .env files.
set -uo pipefail

deployment=${1:?deployment required}
root="${RUNNER_TEMP:-/tmp}/labby-n-minus-one"
command -v cygpath >/dev/null 2>&1 && root=$(cygpath -u "$root")
section() { printf '\n===== %s\n' "$1"; }

for log in "$root"/*/service*.log; do
  [[ -f "$log" ]] || continue
  section "$log"
  tail -n 200 "$log"
done

case "$deployment" in
  host-service)
    section "systemctl status labby.service"
    sudo systemctl status labby.service --no-pager
    section "journalctl -u labby.service"
    sudo journalctl -u labby.service -n 200 --no-pager
    ;;
  incus)
    name="${LABBY_N_MINUS_ONE_INCUS_NAME:-labby-n-minus-one}"
    section "incus list"
    sudo incus list
    section "journalctl -u labby in $name"
    sudo incus exec "$name" -- journalctl -u labby -n 200 --no-pager
    ;;
esac
exit 0
