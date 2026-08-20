---
date: 2026-08-20
repo: git@github.com:dinglebear-ai/labby.git
branch: fix/hardening-followups-20260819
base: 87239104c84874694748ed7135919e11a8d76d4b
worktree: /home/jmagar/workspace/labby-hardening-followups-20260819
---

# Labby cleanup, status, and Microsandbox migration hardening

## Scope

Close the production follow-ups discovered while deploying Depot Skills-over-MCP through Labby: stale Microsandbox cleanup bookkeeping, Gateway Status false-zero warmup snapshots, and legacy mutable Microsandbox image configuration that could make an otherwise healthy service fail on restart.

## Production reproductions before the fix

- Labby repeatedly entered `cleanup_failed` and refused all new Code Mode executions even after Microsandbox fallback removal had physically succeeded or the guest no longer existed. The process-local failed-cleanup ledger did not re-verify reality before opening the creation circuit.
- The same incident left `ACTIVE_SANDBOXES` pinned above reality. Later successful create/remove cycles returned to the same non-zero floor.
- Gateway Status after restart showed upstreams including Depot as `connected: true` with zero tools/capabilities while the lazy catalog was still materializing. Depot later connected with 30 tools and direct calls succeeded.
- The previous production upgrade failed after the binary was installed because `LABBY_CODE_MODE_MICROSANDBOX_IMAGE=debian` was no longer valid once immutable image references became mandatory. Manual digest resolution and canonical cache registration were required.

## Fix contract

1. Failed Microsandbox cleanups are re-verified against the live `labby.owner=codemode` inventory before creation is refused. Proven-absent guests clear the failed record; still-live guests receive one bounded force-removal attempt; inability to verify remains fail closed.
2. The failed-cleanup ledger owns active-count transfer exactly once so fallback cleanup and reconciliation can race without double-decrementing another live guest.
3. Gateway status rereads a catalog when health becomes routable after an empty first snapshot. A still-lazy healthy upstream without a live runtime is explicitly marked `catalog_warming` so zero counts are not presented as authoritative.
4. Host-service install/restart preflights Microsandbox image configuration before stopping the healthy service. Legacy aliases are resolved only from the service user's existing cache, canonicalized to an immutable `docker.io/...@sha256:<64 hex>` reference, registered in cache with bounded `--pull always`, persisted atomically to the exact winning source, and re-verified. If provenance or cache proof is unavailable, restart is refused before disruption.
5. `/home/labby/.labby/.env` values are included because systemd's `show Environment` does not include `EnvironmentFile` contents; explicit `Environment=` drop-ins retain precedence.

## Live host assumptions verified

- Service user: `labby`; home `/home/labby`.
- `runuser -u labby -- env HOME=/home/labby ...` can inspect the Microsandbox cache.
- Alias `debian` and canonical `docker.io/library/debian@sha256:d8f17b92dc7ff10f9c1fdecab0ad21103d1d24aed823c3a0359e4f50adfab3eb` resolve to the same full digest.
- Production was temporarily placed on `LABBY_CODE_MODE_RUNNER_BACKEND=process` only for the maintenance window so the broken old cleanup circuit could not interrupt implementation. The original Microsandbox drop-in was backed up on dookie at `/tmp/labby-microsandbox-codemode.pre-hardening.conf`.

## Verification checklist

- [ ] focused compile/tests
- [ ] clippy with warnings denied
- [ ] broader relevant regression suite
- [ ] docs/diff hygiene
- [ ] PR review and CI
- [ ] merged main build
- [ ] production restore to Microsandbox
- [ ] repeated Code Mode create/remove cycles without cleanup-ledger poisoning
- [ ] immediate post-restart Gateway Status shows `catalog_warming` rather than authoritative false zero where appropriate
- [ ] refreshed Depot catalog exposes 30 tools
- [ ] Depot status/search/load/read live smoke passes through Labby
- [ ] rollback artifacts retired only after a replacement snapshot/binary rollback point is proven

## Post-deploy evidence

Pending final production verification.
