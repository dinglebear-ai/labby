#!/usr/bin/env bash
# Assert this job can mint the GitHub OIDC identity that npm trusted
# publishing authenticates with.
#
# Trusted publishing replaces the npm token, so there is no token identity to
# query and `npm whoami` fails even where publication succeeds. What this
# checks is the half that is knowable before publish time: that the job was
# granted `id-token: write`, so the runner exposes an OIDC token endpoint.
#
# It deliberately does NOT prove that npmjs.com trusts this publisher. Only
# the registry holds that configuration, so a trusted publisher pointed at the
# wrong repository, workflow filename or environment still fails at publish
# time. This check exists to turn the most common and most silent
# misconfiguration -- a missing `id-token: write` permission -- into an early,
# named failure instead of a late publish error.
set -euo pipefail

if [[ -z "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" || -z "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ]]; then
  echo "::error::no OIDC token endpoint; this job needs 'id-token: write' to publish to npm"
  exit 1
fi

# npm speaks OIDC for `publish` from 11.5.1 and for `dist-tag` from 11.21.0.
# Enforce the higher floor everywhere so a job that later gains a dist-tag step
# cannot silently run on a CLI that would fall back to token auth.
required_major=11
required_minor=21
required_patch=0
npm_version="$(npm --version)"
IFS=. read -r major minor patch <<<"${npm_version%%-*}"
if (( major < required_major )) ||
   (( major == required_major && minor < required_minor )) ||
   (( major == required_major && minor == required_minor && patch < required_patch )); then
  echo "::error::npm ${npm_version} cannot publish over OIDC; need >= ${required_major}.${required_minor}.${required_patch}"
  exit 1
fi

echo "OIDC publisher ready: npm ${npm_version}, repository ${GITHUB_REPOSITORY:-unknown}, workflow ${GITHUB_WORKFLOW_REF:-unknown}"
