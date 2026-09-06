---
title: Unified Labby and Depot frontend
created: 2026-09-03
updated: 2026-09-03
---

# Unified Labby and Depot frontend

`gateway-admin` is the only frontend for Labby and Depot. Depot is optional:
when it is disabled, unconfigured, incompatible, or unavailable, every existing
Labby route remains mounted and the Depot screen reports the bounded failure.

## Runtime configuration

Set these only on the Labby server process:

| Variable | Meaning |
| --- | --- |
| `LABBY_DEPOT_ENABLED=1` | Integration kill switch. Omit or set to another value to disable all outbound Depot traffic. |
| `LABBY_DEPOT_URL` | Pinned Depot origin. Redirects are rejected. |
| `LABBY_DEPOT_TOKEN` | Server-held Depot bearer credential. It is never serialized into frontend assets or responses. |

The browser calls only `/v1/depot/*` on Labby. Labby uses one pooled HTTP client
with a five-second connect timeout, fifteen-second request timeout, no redirects,
a 1 MiB JSON response ceiling, and at most sixteen concurrent interactive
requests.

## Authority and supported operations

Discovery remains bounded and read-oriented. The authenticated Administration
surface additionally exposes only operations present in the current actor-filtered,
fingerprinted Depot catalog. A configured credential alone establishes no mutation
authority: Labby reports authority as unknown until attested, revalidates current
browser scope, requires `lab:admin` plus CSRF for mutations, and uses a server-bound
intent for destructive or replay-sensitive execution. Exact Skill Library import
uses the selected provider, artifact, and revision without substitution.

## Static routes and accessibility

The static export uses `/depot` and the query-backed `/depot?artifact=<id>`
detail state. Search has a programmatic label, errors use an alert role, links
and controls retain keyboard focus rings, and status is conveyed with text as
well as color. Pagination uses Depot's generation-bound opaque cursor.

## Qualification and rollback

Before enabling a canary:

1. Run `python3 scripts/check-depot-control-plane-contract.py`.
2. Run `cargo test -p labby --all-features` and the gateway-admin unit/browser suites.
3. Build the static export and scan it for `LABBY_DEPOT_TOKEN` and the live token value.
4. Exercise two distinct users against real Labby and Depot processes; verify cross-user replay, stale revision, cursor-generation, cancellation, oversized response/upload, and outage cases fail closed.
5. Confirm browser network traffic has only the Labby origin and cleanup leaves no uploads, jobs, credentials, or child processes.

Canary one Labby instance and watch Depot latency, rejection, and response-limit
metrics. Roll back immediately by removing
`LABBY_DEPOT_ENABLED=1` and restarting Labby. This is independent of Depot and
does not remove or disable Labby-only routes. Frontend assets and the Labby
binary must be deployed and rolled back as one versioned unit.

Do not mark Phabby superseded until a real-transport qualification, canary, and
rollback rehearsal have produced retained evidence. A local build is not that
evidence.
