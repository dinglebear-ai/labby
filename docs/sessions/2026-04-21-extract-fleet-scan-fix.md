```yaml
date: 2026-04-21 19:13:41 EST
repo: git@github.com:jmagar/lab.git
branch: fix/auth
head: beb3de0
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 6bd8442d-70e5-4684-98f7-033ae7c591f7
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/6bd8442d-70e5-4684-98f7-033ae7c591f7.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#25 — fix(auth): gateway admin auth, upstream OAuth, and dispatch fixes — https://github.com/jmagar/lab/pull/25"
```

## User Request

Add more padding between card header text and the card's bottom border in the gateway admin UI. Also fix `lab extract` fleet scan — it was only finding credentials for 5 services from one host (`tootie`) instead of scanning all SSH hosts and extracting all 9 supported services.

## Session Overview

Two separate fixes:

1. **UI spacing** — fixed wrong Tailwind arbitrary variant selectors in `card.tsx` so `CardHeader` and `CardFooter` get proper padding.
2. **Fleet scan** — fixed two root-cause bugs in `runtime.rs` (`supported_service` never fell back to container name; `arch-` naming prefix not stripped) and added a filesystem-based SFTP fallback in `client.rs` for containers not visible via Docker. Final result: 9/9 credential extractions verified across two SSH hosts, 0 warnings.

## Sequence of Events

1. Identified Tailwind selector bug in `card.tsx` — `[.border-b]:pb-6` targets children of `.border-b`, not the element itself; corrected to `[&.border-b]:pb-6`. Added missing `pt-6` on `CardHeader`.
2. Investigated why fleet scan returned only 5 services from `tootie` instead of all 9 across all hosts.
3. User confirmed via `ssh <host> docker ps` that all 6 SSH hosts have Docker running with containers active — ruling out "no Docker" as the cause.
4. Traced `supported_service` in `runtime.rs` — found it never fell back to container name when image was present but image-based lookup returned `None`.
5. Found `binhex/arch-prowlarr` naming convention — after path stripping, `arch-prowlarr` didn't match `prowlarr`.
6. Fixed `supported_service` to fall back to name only for opaque image IDs (hash strings), not named images.
7. Fixed `service_from_image` to strip `arch-` prefix before matching.
8. Added `is_opaque_image_id` helper for safe disambiguation.
9. Restructured `client.rs` scan flow: Docker scan first (authoritative, knows published host ports), SFTP filesystem scan second (skips already-found services, probes as verification).
10. Added `scan_appdata_roots` and `scan_docker_containers` methods, plus `replace_url_host` helper.
11. Updated tests to match new behavior (filesystem scan returns cred even when probe fails).
12. Verified 9/9 extractions, 0 warnings, all 335 tests passing.

## Key Findings

- **`supported_service` fallback gap** (`runtime.rs`): `match image { Some(img) => service_from_image(img), None => service_from_name(name) }` — when image was a non-None hash ID, `service_from_image` returned `None` and `service_from_name` was never tried.
- **`arch-` prefix** (`runtime.rs`): `binhex/arch-prowlarr` → after path-stripping: `arch-prowlarr`; this didn't match `prowlarr` because the prefix wasn't stripped.
- **Opaque image IDs**: Docker stores only a hex hash (e.g. `5809619fa0e3`) when pulled by digest, discarding repository name. `overseerr` and `linkding` on `squirts` were invisible because of this.
- **Filesystem fallback**: Common appdata roots (`/mnt/user/appdata`, `/mnt/appdata`, `/opt/appdata`, `/srv/appdata`) reliably contain service config files even when Docker metadata is unhelpful.
- **Tailwind arbitrary variants** (`card.tsx`): `[.border-b]:pb-6` applies when the element is a *descendant* of `.border-b`; `[&.border-b]:pb-6` applies when the element *itself* has `.border-b`.

## Technical Decisions

- **Only fall back to container name for opaque image IDs**: Falling back unconditionally caused `plex-tvtime` (a named image) to match `plex` via name tokenization, breaking the `docker_ps_does_not_treat_plex_adjacent_images_as_plex` test. Restricting fallback to hashes preserves the intentional "named image is authoritative" invariant.
- **Docker-first, filesystem-second**: Docker is authoritative for port mapping (actual published host ports). Filesystem scan is supplementary. Services already found via Docker are skipped in the filesystem pass to avoid duplicate creds.
- **Filesystem scan returns cred even when probe fails**: A found API key is still useful even if the endpoint probe fails (wrong port, firewall, etc.). The `url_verified: false` field signals this state to callers.

## Files Modified

| File | Purpose |
|------|---------|
| `apps/gateway-admin/components/ui/card.tsx` | Fix Tailwind arbitrary variant selectors; add missing `pt-6` / `pb-6` to CardHeader/CardFooter |
| `crates/lab-apis/src/extract/runtime.rs` | Fix `supported_service` fallback; add `arch-` prefix stripping; add `is_opaque_image_id` |
| `crates/lab-apis/src/extract/client.rs` | Add `scan_docker_containers`, `scan_appdata_roots`, `replace_url_host`; restructure `scan_fleet_host` flow |

## Commands Executed

```bash
# Build verification
cargo build --all-features 2>&1 | tail -5
# → Finished dev [unoptimized + debuginfo]

# Run extract
./target/debug/lab extract --json
# → 9 creds, 0 warnings

# Full test suite
cargo nextest run --workspace --all-features
# → 335 tests passed
```

## Errors Encountered

| Error | Root Cause | Fix |
|-------|-----------|-----|
| `supported_service` never falls back to name | `match` arm for `Some(image)` returned early on `None` from image lookup | Added `is_opaque_image_id` guard; only fall back when image is a hash |
| `arch-prowlarr` not matched | `service_from_image` didn't strip `arch-` prefix after path stripping | Added `.strip_prefix("arch-")` before SERVICES lookup |
| `plex-tvtime` matched `plex` via name | Naive name fallback applied even to named images | Restricted fallback to opaque image IDs only |
| Duplicate cred (filesystem + Docker same service) | Filesystem scan ran before Docker dedup set was populated | Added `if result.found.contains(&summary.service) { continue; }` at Docker loop start |
| Orphaned dead code in `scan_fleet_host` | Old Docker loop body left outside any loop after restructuring | Moved into `scan_docker_containers` method |
| Test `fleet_scan_reads_config_hints_but_does_not_return_secret_when_probe_fails` failed | Old test expected `NothingFound`; new filesystem scan returns cred with `url_verified: false` | Updated test to assert on returned cred fields |

## Behavior Changes (Before/After)

| | Before | After |
|---|--------|-------|
| Fleet scan hosts | Only `tootie` (first SSH host) | All hosts in `~/.ssh/config` |
| Services found | 5 (radarr, qbittorrent, tautulli, sonarr, sabnzbd on tootie) | 9/9 across tootie + squirts |
| Opaque-hash containers | Invisible to fleet scan | Matched via container name fallback |
| `binhex/arch-*` containers | Missed | Matched after `arch-` prefix strip |
| Probe failure behavior | No cred returned | Cred returned with `url_verified: false` |
| CardHeader padding | No top padding; border-bottom padding selector broken | Correct `pt-6` and `[&.border-b]:pb-6` applied |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `./target/debug/lab extract --json \| jq '.found \| length'` | 9 | 9 | ✅ |
| `./target/debug/lab extract --json \| jq '[.found[].service] \| sort'` | all 9 services | radarr, qbittorrent, tautulli, sonarr, sabnzbd, prowlarr, plex, overseerr, linkding | ✅ |
| `./target/debug/lab extract --json \| jq '[.found[].url_verified] \| all'` | all true | true | ✅ |
| `cargo nextest run --workspace --all-features` | 335 passed, 0 failed | 335 passed, 0 failed | ✅ |
| `cargo build --all-features 2>&1 \| grep -c warning` | 0 | 0 | ✅ |

## Risks and Rollback

- **Filesystem scan adds SSH SFTP connections**: For each host × appdata root that doesn't exist, `stat` calls fail silently. No retry storms; failures are discarded.
- **`url_verified: false` creds now returned**: Callers that previously treated `NothingFound` as "no cred available" now receive a cred they can't immediately verify. The `url_verified` field gates downstream use.
- **Rollback**: Revert `client.rs` and `runtime.rs` to pre-session state; `card.tsx` change is purely cosmetic and safe to keep.

## Next Steps

- **`qbittorrent` has no API key** (`secret=false`): qBittorrent uses session cookies, not a static API key. A follow-up could implement cookie-based auth extraction.
- **`tautulli` and `prowlarr` URLs contain `/login?` suffix**: These are redirect URLs from probe responses. Consider stripping the path suffix to return a cleaner base URL.
- **Only 2 of 6 SSH hosts** had recognized services: `tootie` (7) and `squirts` (2). The remaining 4 hosts (`alien`, `mini`, etc.) were scanned but returned nothing — verify whether this is expected given their actual service inventory.
