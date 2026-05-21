# Service Onboarding Scaffold And Audit Design

## Decision

`lab scaffold service` and `lab audit onboarding` are core `lab` product capabilities.
They are not standalone services, and they are not synthetic `lab-apis` modules.

The product goal is to make service onboarding repeatable and auditable from the
same binary that already owns the CLI, MCP, API, and docs surface.

## Public CLI Surface

- `lab scaffold service <service> [--kind http|non-http] [--dry-run] [--yes/-y]`
- `lab audit onboarding <service...> [--json]`

`scaffold` generates the module tree, patch edits, and coverage doc for a new
service. `audit` checks an onboarding candidate against the repo contract and
reports missing files, missing registration, missing tests, and missing docs.

## MCP Surface

The internal MCP tool is `lab_admin`.

- `onboarding.audit` is exposed over MCP and is read-only.
- `service.scaffold` remains CLI-only in this iteration.

The MCP tool is opt-in and must only register when `LAB_ADMIN_ENABLED=1` is set.
That keeps the audit capability available for automation without exposing a
write-capable onboarding path over MCP before confirmation gating exists.

## Security Posture

- Service names must pass `^[a-z][a-z0-9_]{1,63}$` before any path or subprocess use.
- Scaffold path targets must be validated after join and canonicalization.
- Scaffold writes are destructive and require `--yes` on the CLI.
- Scaffold writes are not reachable over MCP in this iteration.
- Audit is read-only and may be surfaced through `lab_admin` when enabled.

## Implementation Order

1. Scaffold and audit live in `crates/lab/src/`.
2. CLI gets first-class `scaffold` and `audit onboarding` subcommands.
3. Audit is exposed through `lab_admin` only after the CLI path is stable.
4. All-features verification remains the final validation step for onboarding work.
