# Verification toolkit

A reusable Rust correctness-engineering toolkit: an invariant catalog, a
portable scenario format, a replay engine, reporting, and backend adapters.

Labby is the first adopter. Nothing in these crates knows anything about Labby.

The design lives in [docs/plans/verification-toolkit/](../docs/plans/verification-toolkit/README.md).

## Status

M0 — workspace skeleton. The crates compile and CI runs against them; the
vocabulary, envelope, and replay engine arrive in M1 and M2.

## Layout

| Crate | Responsibility |
| --- | --- |
| `verify-core` | invariant identity, catalog and validation, verdicts, `Backend`, `ScenarioTarget` |
| `verify-scenario` | scenario envelope, fingerprint, syntactic normalization |
| `verify-report` | report JSON contract and renderers |
| `verify-runner` | target registry, replay engine, replay-driven normalization, `verify` CLI |

## Why a separate workspace

Backend adapters (Stateright, Kani, Loom, Alloy, TLC) arrive in later milestones
and pull in Java/CBMC-adjacent dependency trees. Keeping them out of the product
workspace's `cargo check --workspace --all-features` is the entire reason this
is its own workspace rather than four more members at the repository root.

The root workspace needs no `exclude` entry: its `members` list is explicit, and
a directory declaring its own `[workspace]` resolves independently. The root
`rust-toolchain.toml` and `clippy.toml` both apply here, so the two workspaces
cannot drift onto different compilers or different lint rules.

## Commands

```bash
just verify-check   # cargo check
just verify-test    # cargo test
just verify-lint    # clippy -D warnings, then fmt --check
just verify-fmt     # cargo fmt
just verify-deny    # cargo deny against this workspace's own lockfile
```

CI runs the same set through a dedicated `verification` routing key, so a change
confined to this directory is actually built and tested rather than skipped.
