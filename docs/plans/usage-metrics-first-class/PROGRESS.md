# Progress

- [x] Reproduced dashboard sampling bug against live telemetry snapshot.
- [x] Confirmed newest 1,000 rows covered about 15.5 minutes while 24h aggregate contained about 48.6k calls.
- [x] Confirmed Usage Explorer also filters/paginates only within newest 1,000 real rows.
- [x] Created isolated worktree from main merge commit 329c4fb61.
- [x] Reconciled the plan with the independently completed first-class usage
  analytics implementation merged in PR #459 (commit 52061b650).
- [x] Backend exact aggregate metrics over the accepted query window, with an
  explicit 250,000-row safety rejection instead of silent sampling.
- [x] Server-side upstream/tool/actor/outcome/search filtering, exact optional
  counts, and keyset call pagination.
- [x] Frontend exact analytics adapter and bounded bucket series.
- [x] Dashboard and Usage Explorer drill-down UX with window/filter context.
- [x] Backend, frontend unit, and browser coverage shipped with PR #459.
- [x] Add first-class capability, operation, and subject-scoped filters across
  params, store queries, facets, CLI/catalog contracts, generated docs, and UI.
- [x] Preserve the full dimensional target identity in slowest-target results.
- [x] Capture representative 24h/7d query plans and timings against a
  production-shaped database; no live database is required for ordinary CI.
- [ ] Decide whether the explicit 250,000-row exact-query ceiling should remain
  fixed, become configurable, or be replaced by a different bounded strategy.
  This remains an explicit policy decision: the current milestone preserves the
  fixed fail-closed ceiling and adds no configuration surface.
- [ ] Run an adversarial review of the remaining milestone when implemented.
