#!/bin/sh
# Run only inside the explicitly selected Ubuntu development/staging sandbox.
set -eu
lock=${1:?usage: bootstrap_ubuntu.sh packages.lock}
export DEBIAN_FRONTEND=noninteractive
set --
missing=0
while IFS= read -r pin || [ -n "$pin" ]; do
    case "$pin" in ''|'#'*) continue;; esac
    case "$pin" in *=*) ;; *) printf '%s\n' 'invalid package lock entry' >&2; exit 2;; esac
    package=${pin%%=*}
    version=${pin#*=}
    [ -n "$package" ] && [ -n "$version" ] || exit 2
    set -- "$@" "$pin"
    installed=$(dpkg-query -W -f='${Version}' "$package" 2>/dev/null || true)
    [ "$installed" = "$version" ] || missing=1
done < "$lock"
[ "$#" -gt 0 ] || { printf '%s\n' 'package lock is empty' >&2; exit 2; }
if [ "$missing" -eq 1 ]; then
    apt-get update -o Acquire::Retries=2 -o Acquire::http::Timeout=15
    apt-get install --yes --no-install-recommends -- "$@"
fi
for pin do
    package=${pin%%=*}
    version=${pin#*=}
    [ "$(dpkg-query -W -f='${Version}' "$package")" = "$version" ] || exit 1
done
command -v git >/dev/null
command -v python3 >/dev/null
mkdir -p /workspace
printf '%s\n' 'bootstrap_verified'
