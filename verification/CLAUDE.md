# verification/ — Verification Toolkit Rules

This tree is a **separate Cargo workspace** from the product workspace at the
repository root. It is not a Labby product surface and must not become one.

The design contract is
[docs/plans/verification-toolkit/SPEC.md](../docs/plans/verification-toolkit/SPEC.md).
Read it before changing a public type here; the milestones are in
[IMPLEMENTATION.md](../docs/plans/verification-toolkit/IMPLEMENTATION.md).

## Layering

Three layers, strictly ordered, each depending only on the ones above it:

1. **Infrastructure** — catalog schema, scenario format, normalization, replay,
   reporting, CI helpers, backend adapters. Lives here.
2. **Domain patterns** — reusable state-machine patterns. Lives here, opt-in.
3. **Project models** — concrete state machines, invariant catalogs, formal
   specifications. Lives in the adopting project, never here.

**No domain vocabulary in layer 1.** There is no `upstream`, `mount`, or
`session` type in these crates. A change to layer 1 needed to support one
project's domain is a design bug in layer 1 — fix the abstraction or keep the
behavior in layer 3.

## Crate Boundaries

| Crate | May depend on |
| --- | --- |
| `verify-core` | `serde`, `serde_json`, `toml`, `thiserror` — data formats only, no filesystem, no env, no transport |
| `verify-scenario` | `verify-core` |
| `verify-report` | `verify-core`, `verify-scenario` |
| `verify-runner` | `verify-core`, `verify-scenario`, `verify-report` |

`verify-runner` depends on **no backend crate**. The `Backend` trait lives in
`verify-core` and adopting projects register implementations themselves. That
inversion is what lets a project use Stateright without installing a Java
toolchain; do not collapse it for convenience.

`verify-scenario` must never depend on `verify-runner`. Replay-driven work
(prefix minimization, the determinism check) belongs in the runner precisely
because the reverse direction is a cycle.

## Honest Verdicts

`Bounded` is not `Verified`, `Skipped` is not `Error`, and an invariant no
backend claims is reported `Uncovered` by id rather than quietly omitted. A
report that overstates assurance is worse than no report — most of this
toolkit's value is that its output can be trusted literally.

## Rules Inherited From The Repository Root

- `rust-toolchain.toml` and `clippy.toml` at the root apply here. Do not add
  local copies.
- No `mod.rs`. Use `foo.rs` plus a sibling `foo/` directory.
- Native async traits only; `#[async_trait]` is banned workspace-wide.
- `[workspace.lints.clippy]` in `verification/Cargo.toml` mirrors the root
  table. Keep them in sync rather than letting the two workspaces diverge.

## Adding A Dependency

Prefer a version already pinned in the root `Cargo.toml` so the two workspaces
agree.

The root `Cargo Deny` job reads only the root lockfile, so this workspace has its
own `deny.toml` and the `verification` CI job runs `cargo deny` against it. The
two policy files are separate on purpose — the root `[[bans.deny]]` entries
whitelist product crates as wrappers and would reject `verify-runner`'s
legitimate use of clap — but the **license allowlists are meant to be identical**.
Keep them in sync when either changes. A dependency this workspace cannot
license-clear does not land here either.
