#!/bin/sh
set -eu

COMPOSE_ENV_FILE=${COMPOSE_ENV_FILE:-.env}
TEAM_DEPOT_SERVICE=${TEAM_DEPOT_SERVICE:-team-depot}
TEAM_DEPOT_GIT_CREDENTIAL_REF=${TEAM_DEPOT_GIT_CREDENTIAL_REF:-github-private}

run_depotctl() {
  if [ -n "${TEAM_DEPOTCTL:-}" ]; then
    "$TEAM_DEPOTCTL" "$@"
  else
    docker compose --env-file "$COMPOSE_ENV_FILE" exec -T "$TEAM_DEPOT_SERVICE" /app/bin/depotctl "$@"
  fi
}

ingest_repo() {
  namespace=$1
  url=$2
  arguments=$(printf '{"url":"%s","namespace":"%s","credential":"%s"}' "$url" "$namespace" "$TEAM_DEPOT_GIT_CREDENTIAL_REF")
  run_depotctl --json op depot.skills.ingest_repo --args-json "$arguments"
}

ingest_repo unmarket https://github.com/unraid/unmarket
ingest_repo limetech-ai-skills https://github.com/unraid/limetech-ai-skills
ingest_repo limetech-elixir-skills https://github.com/unraid/limetech-elixir-skills

run_depotctl --json sources list
run_depotctl --json status
