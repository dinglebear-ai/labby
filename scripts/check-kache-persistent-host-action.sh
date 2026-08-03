#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

action=.github/actions/setup-rust-kache/action.yml
detect=.github/actions/setup-rust-kache/detect-persistent-host.sh
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

[ -x "$detect" ]
grep -Fq 'id: kache-host' "$action"
grep -Fq 'run: "$GITHUB_ACTION_PATH/detect-persistent-host.sh"' "$action"
grep -Fq "steps.kache-host.outputs.managed == 'true'" "$action"
grep -Fq "steps.kache-host.outputs.managed != 'true'" "$action"
grep -Fq 'uses: kunobi-ninja/kache-action@a257c055543c2840700a9bbca8f9c3094a421b1b' "$action"
grep -Fq 's3-prefix: rust' "$action"
grep -Fq 's3-endpoint: https://s3.tootie.tv' "$action"
grep -Fq 'binary="${{ steps.kache-host.outputs.binary }}"' "$action"
grep -Fq 'echo "RUSTC_WRAPPER=$binary"' "$action"
grep -Fq 'echo "CARGO_BUILD_RUSTC_WRAPPER=$binary"' "$action"

mkdir -p "$tmpdir/bin" "$tmpdir/home/.config/kache"
cat >"$tmpdir/bin/systemctl" <<'SH'
#!/usr/bin/env bash
if [ "${FAKE_SERVICE_ACTIVE:-0}" = 1 ] &&
   [ "$*" = '--user is-active --quiet kache.service' ]; then
  exit 0
fi
exit 3
SH
cat >"$tmpdir/bin/kache" <<'SH'
#!/usr/bin/env bash
if [ "$*" = 'daemon status' ]; then
  if [ "${FAKE_DAEMON_RUNNING:-0}" = 1 ]; then
    printf '  Daemon:   running\n'
  else
    printf '  Daemon:   not running\n'
  fi
  exit 0
fi
exit 2
SH
chmod +x "$tmpdir/bin/systemctl" "$tmpdir/bin/kache"

run_case() {
  local name="$1" service="$2" daemon="$3" type="$4" prefix="$5" expected="$6"
  local output="$tmpdir/$name.out" actual
  cat >"$tmpdir/home/.config/kache/config.toml" <<EOF
[cache.remote]
type = "$type"
prefix = "$prefix"
EOF
  : >"$output"
  PATH="$tmpdir/bin:/usr/bin:/bin" \
  HOME="$tmpdir/home" \
  GITHUB_OUTPUT="$output" \
  KACHE_MANAGED_BINARY="$tmpdir/bin/kache" \
  FAKE_SERVICE_ACTIVE="$service" \
  FAKE_DAEMON_RUNNING="$daemon" \
    "$detect"
  actual="$(sed -n 's/^managed=//p' "$output")"
  detected_binary="$(sed -n 's/^binary=//p' "$output")"
  [ "$detected_binary" = "$tmpdir/bin/kache" ]
  [ "$actual" = "$expected" ] || {
    echo "FAIL: $name expected managed=$expected, got $actual" >&2
    exit 1
  }
}

run_case managed-s3 1 1 s3 rust true
run_case inactive-service 0 1 s3 rust false
run_case stopped-daemon 1 0 s3 rust false
run_case filesystem-remote 1 1 filesystem rust false
run_case wrong-prefix 1 1 s3 canary false

python3 - <<'PY'
from pathlib import Path
text = Path('.github/actions/setup-rust-kache/action.yml').read_text()
order = [
    text.index('- name: Detect persistent host Kache'),
    text.index('- name: Reuse persistent host Kache'),
    text.index('- name: Connect shared MinIO Kache'),
    text.index('- name: Use bare Cargo when shared credentials are unavailable'),
    text.index('- name: Print cache evidence'),
]
assert order == sorted(order), order

def route(enabled: bool, linux: bool, managed: bool, credentials: bool) -> str:
    if enabled and linux and managed:
        return 'persistent'
    if enabled and linux and not managed and credentials:
        return 'hosted-action'
    return 'bare'

cases = {
    (True, True, True, False): 'persistent',
    (True, True, True, True): 'persistent',
    (True, True, False, True): 'hosted-action',
    (True, True, False, False): 'bare',
    (False, True, True, True): 'bare',
    (True, False, False, True): 'bare',
}
for inputs, expected in cases.items():
    actual = route(*inputs)
    assert actual == expected, (inputs, actual, expected)
PY

echo 'check-kache-persistent-host-action: OK'
