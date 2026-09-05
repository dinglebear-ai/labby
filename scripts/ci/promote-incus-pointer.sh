#!/usr/bin/env bash
set -euo pipefail

mode=${1:?promote or rollback required}
release_tag=${RELEASE_TAG:?RELEASE_TAG is required}
rolling_tag=${INCUS_ROLLING_TAG:-labby-incus-latest}
receipt=${INCUS_POINTER_RECEIPT:?INCUS_POINTER_RECEIPT is required}
gh_bin=${GH_BIN:-gh}
remote="https://x-access-token:${GH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"

write_state() {
  printf '%s\n' "$1" >"$receipt/state.tmp"
  mv "$receipt/state.tmp" "$receipt/state"
}

remote_target() { git ls-remote origin "refs/tags/$rolling_tag" | awk '{print $1}'; }
push_with_lease() {
  local target=$1 expected=$2
  git tag -f "$rolling_tag" "$target"
  git push --force-with-lease="refs/tags/$rolling_tag:$expected" "$remote" "refs/tags/$rolling_tag:refs/tags/$rolling_tag"
  [[ $(remote_target) == "$target" ]] || { echo "rolling tag remote verification failed" >&2; return 1; }
}
verify_generation() {
  python3 - "$1" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / "release-manifest.json").read_text())
for subject in manifest["subjects"]:
    for item in (subject, subject["sbom"]):
        path = root / item["name"]
        if not path.is_file() or path.stat().st_size != item["size"] or hashlib.sha256(path.read_bytes()).hexdigest() != item["sha256"]:
            raise SystemExit(f"release subject mismatch: {item['name']}")
incus = manifest["distributions"]["incus"]
path = root / incus["asset"]
if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != incus["sha256"]:
    raise SystemExit("Incus distribution mismatch")
PY
}

case "$mode" in
  promote)
    mkdir -p "$receipt/candidate"
    remote_target >"$receipt/previous-target"
    write_state "prepared"
    "$gh_bin" release download "$release_tag" --dir "$receipt/candidate"
    verify_generation "$receipt/candidate"
    manifest_digest=$(sha256sum "$receipt/candidate/release-manifest.json" | awk '{print $1}')
    printf '{"release_tag":"%s","git_sha":"%s","release_manifest_sha256":"%s"}\n' "$release_tag" "$GITHUB_SHA" "$manifest_digest" >"$receipt/candidate/generation.json"
    # Assets remain in the immutable versioned release namespace. The rolling
    # Git ref is the only mutable pointer and is changed with one leased CAS.
    mkdir -p "$receipt/existing-generation"
    if "$gh_bin" release download "$release_tag" --pattern generation.json --dir "$receipt/existing-generation" 2>/dev/null; then
      cmp "$receipt/candidate/generation.json" "$receipt/existing-generation/generation.json"
    else
      "$gh_bin" release upload "$release_tag" "$receipt/candidate/generation.json"
    fi
    push_with_lease "$GITHUB_SHA" "$(<"$receipt/previous-target")"
    write_state "promoted"
    ;;
  rollback)
    state=$(<"$receipt/state")
    current=$(remote_target)
    previous=$(<"$receipt/previous-target")
    if [[ $state == prepared && $current == "$previous" ]]; then
      write_state "rolled-back"
      exit 0
    fi
    [[ ($state == prepared || $state == promoted) && $current == "$GITHUB_SHA" ]] || { echo "rolling pointer changed after promotion; refusing stale rollback" >&2; exit 75; }
    if [[ -n $previous ]]; then
      push_with_lease "$previous" "$GITHUB_SHA"
    else
      git push --force-with-lease="refs/tags/$rolling_tag:$GITHUB_SHA" "$remote" ":refs/tags/$rolling_tag"
      [[ -z $(remote_target) ]] || { echo "rolling pointer deletion verification failed" >&2; exit 1; }
    fi
    write_state "rolled-back"
    ;;
  *) exit 64 ;;
esac
