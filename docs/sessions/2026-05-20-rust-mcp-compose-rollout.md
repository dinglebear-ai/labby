---
date: 2026-05-20 23:24:37 EDT
repo: git@github.com:jmagar/lab.git
branch: fix/docker-network-default
head: 11a51d37
session id: 03d30e77-674f-4748-b0cc-59aa73fc9a27
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/03d30e77-674f-4748-b0cc-59aa73fc9a27.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab 11a51d37 [fix/docker-network-default]
pr: #66 fix(compose): use repo-name default for Docker network https://github.com/jmagar/lab/pull/66
---

# Rust MCP Compose Rollout

## User Request

Bring down the OpenMemory MCP stack, then update the Rust MCP server Docker Compose files to follow the Axon compose pattern: dev builds/runs locally, prod uses the published image. After clarification, apply the same pattern to Lab itself as well.

## Session Overview

- Stopped the self-hosted OpenMemory/OpenMemory Axon compose stack.
- Split dev/prod compose files across the Rust MCP repos: `unrust`, `rustscale`, `apprise-mcp`, `rustifi`, `rustify`, `syslog-mcp`, and `rmcp-template`.
- Redeployed the services from local dev images and verified Docker health plus host `/health` endpoints.
- Updated `lab` to the Axon-style two-file pattern: `docker-compose.prod.yml` for the GHCR image and `docker-compose.yml` for the dev runtime with local binary/assets bind mounts.
- Committed and pushed all in-scope repositories; current state includes follow-up `fix/docker-network-default` commits on the relevant repos.

## Sequence of Events

1. Located running OpenMemory containers from Docker labels and brought down `/home/jmagar/workspace/mem0/openmemory/docker-compose.axon.yml`, then also removed the older stale `openmemory` compose stack.
2. Reviewed Axon’s compose files and identified the intended pattern: `docker-compose.prod.yaml` owns production runtime/image definitions, while `docker-compose.yaml` extends prod and swaps in a dev runtime with bind-mounted local binary output.
3. Added `docker-compose.prod.yml` and converted `docker-compose.yml` to local-build dev stacks in each Rust MCP server repo.
4. Redeployed all services with `docker compose up --build -d`, fixed `rmcp-template` build/runtime issues, and verified all service containers and endpoints.
5. Committed and pushed all touched repos, including `apprise-mcp`.
6. Updated Lab after user pointed out it had been missed, then corrected Lab again to remove the stale third `docker-compose.dev.yml` layer and fold hot-swap behavior into `docker-compose.yml`, matching Axon’s two-file shape.
7. Observed current follow-up branch state: Lab and most Rust MCP repos now sit on `fix/docker-network-default`; `syslog-mcp` remains on `feat/syslog-self-debugging-ergonomics`.

## Key Findings

- Axon dev compose does hot-swap a local binary: `/home/jmagar/workspace/axon_rust/docker-compose.yaml` bind-mounts `${AXON_DEV_TARGET_DIR:-./target/debug}` into `/home/axon/.axon/dev` and sets the entrypoint to `/home/axon/.axon/dev/axon`.
- The first Lab split preserved a separate `docker-compose.dev.yml`, which did not match Axon’s file shape. This was corrected by deleting `docker-compose.dev.yml` and moving `./bin/labby:/usr/local/bin/labby` plus `LAB_WEB_ASSETS_DIR=/workspace/lab/apps/gateway-admin/out` into `docker-compose.yml`.
- `rmcp-template` was the only rollout that needed build/runtime repair: stale `apps/web/pnpm-lock.yaml`, missing `docs/generated/openapi.json` in the Docker context/image, bind-mount ownership chmod/chown assumptions, and loopback-only bind defaults for a published Docker port.
- Current compose image resolution verifies the intended split: dev files resolve to local `*:dev` images; prod files resolve to GHCR images.
- `rmcp-template` remains a template and its prod image still intentionally resolves to `ghcr.io/your-org/example-mcp:latest`.

## Technical Decisions

- Kept production service settings in `docker-compose.prod.yml` so deploys have a single GHCR-backed runtime definition.
- Kept dev compose files extending prod so ports, volumes, restart policy, and runtime hardening do not drift between dev and prod.
- Matched Axon’s approach for Lab by keeping hot-swap behavior in the main dev compose file instead of a separate `.dev` override.
- Left unrelated dirty work alone during the initial rollout unless the user later requested staging all changes in the in-scope repos.
- Preserved `rmcp-template`’s placeholder GHCR image because it is a template repo, not a concrete deployed service image.

## Files Modified

- `lab/docker-compose.yml`: dev stack now extends prod, uses `labby:dev`, builds `config/Dockerfile.fast`, and bind-mounts `./bin/labby` plus workspace web assets.
- `lab/docker-compose.prod.yml`: production/runtime Lab service definition using `${LAB_IMAGE:-ghcr.io/jmagar/lab:latest}`.
- `lab/docker-compose.dev.yml`: removed after folding hot-swap behavior into `docker-compose.yml`.
- `lab/Justfile`: `dev-up`, `dev`, and `dev-debug` now use only `docker-compose.yml`.
- `lab/README.md` and `lab/CLAUDE.md`: updated Docker dev-container wording to describe the new two-file pattern.
- `unrust`, `rustscale`, `apprise-mcp`, `rustifi`, `rustify`, `syslog-mcp`, `rmcp-template`: added or updated dev/prod compose files.
- `rmcp-template/.dockerignore`, `rmcp-template/config/Dockerfile`, `rmcp-template/entrypoint.sh`, `rmcp-template/apps/web/pnpm-lock.yaml`: fixed Docker build and runtime startup.

## Commands Executed

- `docker compose -f /home/jmagar/workspace/mem0/openmemory/docker-compose.axon.yml down`: stopped the OpenMemory Axon stack.
- `docker compose -f /home/jmagar/workspace/mem0/openmemory/docker-compose.yml down`: removed stale older OpenMemory containers.
- `docker compose -f <repo>/docker-compose.yml config --quiet`: validated dev compose files.
- `docker compose -f <repo>/docker-compose.prod.yml config --quiet`: validated prod compose files.
- `docker compose up --build -d`: rebuilt and redeployed local dev images for the Rust MCP services.
- `docker compose -f docker-compose.yml up -d labby-master`: recreated Lab from the corrected single dev compose file.
- `git add -A`, `git commit`, `git push`: committed and pushed all requested repos.

## Errors Encountered

- `rmcp-template` Docker build failed on a stale PNPM lockfile. Resolved with `corepack pnpm install --lockfile-only` in `apps/web`.
- `rmcp-template` Docker build failed because `.dockerignore` excluded `docs/generated/openapi.json` while Rust used `include_str!`. Resolved by allowing that generated file through `.dockerignore` and copying it in `config/Dockerfile`.
- `rmcp-template` container restarted because `entrypoint.sh` treated bind-mount `chown` and `chmod` failures as fatal. Resolved by making those operations warn and continue.
- `rmcp-template` container failed under `no-new-privileges` because `gosu` could not switch users. Resolved by running the compose service as `1000:1000` and making the entrypoint skip `gosu` when already non-root.
- `rmcp-template` host `/health` initially reset because the server bound `127.0.0.1` inside the container. Resolved with `EXAMPLE_MCP_HOST=0.0.0.0` and `EXAMPLE_NOAUTH=true` in compose.
- Several shell endpoint loops initially failed in `zsh` because unquoted strings were not split as intended. Re-ran under `bash -lc` for valid endpoint evidence.
- Lab’s first update kept `docker-compose.dev.yml`; user pointed out Axon was the original pattern. Resolved by folding hot-swap mounts/env into `docker-compose.yml` and deleting the override.

## Behavior Changes (Before/After)

| Area | Before | After |
| --- | --- | --- |
| Rust MCP dev compose | Mixed image/build definitions, some dev files could still imply pulling images | Dev compose resolves to local `*:dev` images and builds from checkout |
| Rust MCP prod compose | Missing or inconsistent prod-only files | Prod compose resolves to GHCR image paths |
| Lab dev compose | Initially full runtime in `docker-compose.yml`; then a three-file dev setup | Axon-style two-file setup: prod file for GHCR, dev file for local runtime and hot-swap |
| Lab hot-swap | `docker-compose.dev.yml` mounted `./bin/labby` and web assets | `docker-compose.yml` now mounts `./bin/labby` and sets `LAB_WEB_ASSETS_DIR` directly |
| OpenMemory | OpenMemory Axon compose stack was running | No `openmemory`/`mem0` containers remained after shutdown verification |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `docker ps -a ... | rg -i 'openmemory|mem0'` | No OpenMemory/Mem0 containers | No output after stack shutdown | Pass |
| `docker compose -f docker-compose.yml config --images` in Lab | `labby:dev` | `labby:dev` | Pass |
| `docker compose -f docker-compose.prod.yml config --images` in Lab | `ghcr.io/jmagar/lab:latest` | `ghcr.io/jmagar/lab:latest` | Pass |
| `docker compose config --images` across Rust MCP dev files | Local `*:dev` images | `unrust:dev`, `rustscale:dev`, `apprise-mcp:dev`, `rustifi:dev`, `rustify:dev`, `syslog-mcp:dev`, `rmcp-template:dev` | Pass |
| `docker compose config --images` across Rust MCP prod files | GHCR images | `ghcr.io/jmagar/...:latest`; template uses `ghcr.io/your-org/example-mcp:latest` | Pass |
| `curl http://127.0.0.1:8765/health` | Lab status ok | `{"status":"ok","mode":"master","pid":7,"uptime_s":3317}` | Pass |
| Rust MCP `/health` endpoints | Status ok | Unraid, Tailscale, Apprise, Unifi, Gotify, Example, and Syslog returned status ok | Pass |
| `docker ps` service images | Running local dev images | Lab and seven MCP containers ran `*:dev` images; MCP containers healthy | Pass |
| `git rev-list --left-right --count HEAD...@{u}` during push verification | `0 0` | Repos reported synced after pushes | Pass |

## Risks and Rollback

- Lab now depends on `bin/labby` existing for the dev container because the dev compose bind-mounts it over the image binary. Roll back by restoring `docker-compose.dev.yml` or removing the `./bin/labby:/usr/local/bin/labby` mount.
- `rmcp-template` prod remains a template and should not be deployed as-is without replacing `ghcr.io/your-org/example-mcp`.
- `syslog-mcp` had an unrelated dirty `CLAUDE.md` when this note was written; do not assume that file was part of the compose rollout.
- Rollback for the compose split is straightforward per repo: revert the compose commits and redeploy the prior compose file.

## Decisions Not Taken

- Did not add hot-swap override files to the other Rust MCP repos. The user asked to copy Axon’s pattern, and Axon folds hot-swap behavior into the main dev compose file.
- Did not force prod compose to build locally. Prod should remain image-based by design.
- Did not replace `rmcp-template`’s placeholder prod image because the repository is explicitly a template.

## References

- Axon compose reference: `/home/jmagar/workspace/axon_rust/docker-compose.yaml` and `/home/jmagar/workspace/axon_rust/docker-compose.prod.yaml`.
- OpenMemory memory/context: self-hosted OpenMemory under `/home/jmagar/workspace/mem0/openmemory/api`, launched from `openmemory/docker-compose.axon.yml`.
- Lab PR observed at save time: https://github.com/jmagar/lab/pull/66.

## Open Questions

- Whether the follow-up `fix/docker-network-default` branches in all repos should be merged to main immediately or remain PR-scoped.
- Whether `syslog-mcp`’s dirty `CLAUDE.md` is intentional and should be committed separately.
- Whether the other Rust MCP repos should eventually get Axon-style bind-mounted debug binaries instead of local image rebuild dev loops.

## Next Steps

- Started but not completed: none in Lab; the save note itself is a new uncommitted file.
- Follow-on: review and merge PR #66 if the network-default change is ready.
- Follow-on: decide what to do with the dirty `syslog-mcp/CLAUDE.md`.
- Follow-on: replace `rmcp-template` placeholder prod image when generating a real server from the template.
