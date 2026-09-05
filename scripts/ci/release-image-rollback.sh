#!/usr/bin/env bash
set -u

image=""
tag=""
previous_latest="none"
previous_release="none"
delete_version_id=""
while (($#)); do
  case "$1" in
    --image) image=$2; shift 2 ;;
    --tag) tag=$2; shift 2 ;;
    --previous-latest) previous_latest=$2; shift 2 ;;
    --previous-release) previous_release=$2; shift 2 ;;
    --delete-version-id) delete_version_id=$2; shift 2 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 64 ;;
  esac
done

[[ -n "$image" && -n "$tag" ]] || { echo 'image and tag are required' >&2; exit 64; }
gh_bin=${GH_BIN:-gh}
docker_bin=${DOCKER_BIN:-docker}
failures=0
delete_status=skipped
restore_status=skipped
verify_status=skipped
release_absent_status=skipped
restore_release_status=skipped
verify_release_status=skipped
verify_attempts=${ROLLBACK_VERIFY_ATTEMPTS:-10}
verify_delay=${ROLLBACK_VERIFY_DELAY_SECONDS:-2}

verify_release_absent() {
  local attempt output
  for ((attempt = 1; attempt <= verify_attempts; attempt++)); do
    if output=$("$docker_bin" buildx imagetools inspect "$image:$tag" 2>&1); then
      : # The tag still resolves; continue polling.
    elif grep -Eiq 'not found|manifest unknown|no such manifest' <<<"$output"; then
      return 0
    else
      # Authentication, transport, and lookup failures are not absence proof.
      return 2
    fi
    ((attempt == verify_attempts)) || sleep "$verify_delay"
  done
  return 1
}

# Recovery is deliberately best-effort per step and fail-closed as a whole.
# One failed operation must never prevent later restoration or verification.
if [[ -n "$delete_version_id" ]]; then
  image_path=${image#ghcr.io/}
  image_owner=${image_path%%/*}
  if "$gh_bin" api --method DELETE "/users/$image_owner/packages/container/${image##*/}/versions/$delete_version_id"; then
    delete_status=ok
  else
    delete_status=failed
    failures=$((failures + 1))
  fi
fi

# A retry may have found this immutable version already present. Restore and
# prove that exact digest; only versions created by this run must disappear.
if [[ "$previous_release" == sha256:* ]]; then
  if "$docker_bin" buildx imagetools create --tag "$image:$tag" "$image@$previous_release"; then
    restore_release_status=ok
  else
    restore_release_status=failed
    failures=$((failures + 1))
  fi
  observed=$("$docker_bin" buildx imagetools inspect "$image:$tag" --format '{{json .Manifest.Digest}}' 2>/dev/null) || observed=""
  observed=${observed//\"/}
  if [[ "$observed" == "$previous_release" ]]; then
    verify_release_status=ok
  else
    verify_release_status=failed
    failures=$((failures + 1))
  fi
elif verify_release_absent; then
  release_absent_status=ok
else
  release_absent_status=failed
  failures=$((failures + 1))
fi

if [[ "$previous_latest" == sha256:* ]]; then
  if "$docker_bin" buildx imagetools create --tag "$image:latest" "$image@$previous_latest"; then
    restore_status=ok
  else
    restore_status=failed
    failures=$((failures + 1))
  fi
  observed=""
  for ((attempt = 1; attempt <= verify_attempts; attempt++)); do
    observed=$("$docker_bin" buildx imagetools inspect "$image:latest" --format '{{json .Manifest.Digest}}' 2>/dev/null) || observed=""
    observed=${observed//\"/}
    [[ "$observed" == "$previous_latest" ]] && break
    ((attempt == verify_attempts)) || sleep "$verify_delay"
  done
  if [[ "$observed" == "$previous_latest" ]]; then
    verify_status=ok
  else
    verify_status=failed
    failures=$((failures + 1))
  fi
fi

status_file=${ROLLBACK_STATUS_FILE:-}
python3 - "$failures" "$delete_status" "$release_absent_status" "$restore_release_status" "$verify_release_status" "$restore_status" "$verify_status" "$status_file" <<'PY'
import json, sys
failures = int(sys.argv[1])
payload = json.dumps({
    "status": "ok" if failures == 0 else "failed",
    "steps": {
        "delete_version": sys.argv[2],
        "release_tag_absent": sys.argv[3],
        "restore_release": sys.argv[4],
        "verify_release": sys.argv[5],
        "restore_latest": sys.argv[6],
        "verify_latest": sys.argv[7],
    },
}, sort_keys=True)
print(payload)
if sys.argv[8]:
    with open(sys.argv[8], "w", encoding="utf-8") as handle:
        handle.write(payload + "\n")
PY
((failures == 0))
