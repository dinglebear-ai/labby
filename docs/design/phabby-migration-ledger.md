---
title: "Phabby Migration Ledger"
status: active-plan
created: "2026-08-22"
---

# Phabby Migration Ledger

This ledger prevents the accepted Phabby target from being mistaken for current
Labby behavior. Every route remains on its legacy implementation until its
acceptance and rollback evidence is attached.

## Architecture waves

| Wave | Deliverable | Current state | Exit gate |
| --- | --- | --- | --- |
| 0 | Ownership, licensing, history, trademark, contribution, and assets | in progress | recorded approval allows source movement |
| 1 | Phabby Phoenix/OTP repository and architectural boundary | bootstrapped privately | clean build, tests, release, docs, and private remote |
| 2 | Backend behaviours, capabilities, sessions, errors, and compatibility fixtures | planned | Rust/Elixir fixtures and conformance tests agree |
| 3 | Versioned Rust-to-BEAM local RPC and event stream | planned | auth, bounds, redaction, reconnect, health, and isolation verified |
| 4 | Shared shell and Labby read-only routes | planned | per-route parity and rollback proof |
| 5 | Labby mutations and Depot read/mutation workflows | planned | backend authorization and failure journeys pass |
| 6 | Connected Depot and Send-to-Labby | planned | target-bound transfer, audit, integrity, retry, and partial-state proof |
| 7 | Bundled BEAM release in native and Incus packaging | planned | paired upgrade, rollback, readiness, and reboot proof |
| 8 | Legacy React/static export retirement | planned | route ledger empty and rollback rehearsed |
| 9 | Optional responsibility-level Rust-to-Elixir migration | deferred | separate decision and production-equivalent evidence |

## Initial route inventory

The first executable snapshot now lives in Phabby at
`priv/contracts/migration/routes-v1.json`. It records 21 Labby page routes and
9 Depot LiveView routes against immutable source revisions, including each
legacy source, proposed target path and owner, auth boundary, product modes,
rendering strategy, and migration state. Phabby validates its schema and checks
it for drift against live backend checkouts with:

```console
mix phabby.routes.check --labby /path/to/labby --depot /path/to/depot
```

The snapshot still must expand to callbacks, deep links, per-route API
dependencies, asset provenance, failure states, and acceptance evidence before
UI source moves. The grouped table below remains a planning summary, not the
machine-authoritative inventory.

| Route group | Current owner | Target | State |
| --- | --- | --- | --- |
| overview | Labby React static export | Phabby LiveView | legacy authoritative |
| gateways and detail | Labby React static export | Phabby LiveView plus measured JS islands | legacy authoritative |
| snippets | Labby React static export | Phabby LiveView | legacy authoritative |
| usage and traces | Labby React static export | Phabby LiveView streams | legacy authoritative |
| settings, setup, doctor | Labby React static export | Phabby LiveView | legacy authoritative |
| docs and design system | Labby React static export | Phabby routes | legacy authoritative |
| Code Mode inspector | Labby React static export | Phabby with client island | legacy authoritative |
| Depot Discovery and Library | Depot LiveView | Phabby LiveView | legacy authoritative |
| Depot create, publish, teams, admin | Depot LiveView | Phabby LiveView | legacy authoritative |

Each expanded entry records modes, capabilities, backend operations,
authentication and tenant scope, failure/reconnect states, accessibility and
responsive evidence, telemetry, compatibility, deployment, canary ownership,
rollback, and deletion gates.

Phabby bootstrap does not authorize moving Labby responsibilities from Rust.
Each proposed move needs a separate decision naming why OTP is the better owner,
its data and supervision model, failure blast radius, performance budget,
contract, migration mechanism, and rollback path.
