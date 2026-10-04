#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
fixture_root=$(mktemp -d)
trap 'rm -rf "$fixture_root"' EXIT
mkdir "$fixture_root/bin"
printf 'archive bytes' >"$fixture_root/archive.tar.gz"
cat >"$fixture_root/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == --version ]]; then echo "gh version 2.102.0"; exit 0; fi
if [[ "$*" == "attestation verify --help" ]]; then exit 0; fi
case "$1 $2" in
  'attestation download')
    [[ ${GH_TOKEN:-} == fixture-publisher ]] || exit 70
    printf '{"fixture":"signed"}\n' >'sha256:fixture.jsonl'
    ;;
  'attestation verify')
    [[ -z ${GH_TOKEN:-} && -z ${GITHUB_TOKEN:-} && -d ${GH_CONFIG_DIR:-} ]] || exit 71
    [[ "$*" == *'--repo example/labby'* && "$*" == *'--source-ref refs/tags/v1.2.3'* && "$*" == *'--deny-self-hosted-runners'* && "$*" == *'--bundle '* ]] || exit 72
    exit "${FIXTURE_VERIFY_FAILURE:-0}"
    ;;
  *) exit 73 ;;
esac
EOF
chmod 755 "$fixture_root/bin/gh"
run_export() {
  PATH="$fixture_root/bin:$PATH" GH_TOKEN=fixture-publisher GITHUB_TOKEN=fixture-publisher \
    GITHUB_REPOSITORY=example/labby RELEASE_TAG=v1.2.3 \
    "$repo_root/scripts/ci/export-release-attestation-bundles.sh" "$fixture_root/archive.tar.gz"
}
run_export
[[ -s "$fixture_root/archive.tar.gz.sigstore.jsonl" ]] || { echo 'missing verified bundle' >&2; exit 1; }
rm "$fixture_root/archive.tar.gz.sigstore.jsonl"
if FIXTURE_VERIFY_FAILURE=1 run_export; then
  echo 'failed verification was accepted' >&2; exit 1
fi
[[ ! -e "$fixture_root/archive.tar.gz.sigstore.jsonl" ]] || { echo 'failed verification published bundle' >&2; exit 1; }
echo 'release bundle export behavioral tests passed'
