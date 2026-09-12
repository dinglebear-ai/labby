# Verification toolkit

A reusable Rust correctness-engineering toolkit: an invariant catalog, a
portable scenario format, a replay engine, reporting, and backend adapters.

Labby is the first adopter. Nothing in these crates knows anything about Labby.

The design lives in [docs/plans/verification-toolkit/](../docs/plans/verification-toolkit/README.md).

## Status

M0–M2 are implemented: the independent workspace and CI routing, invariant
catalog and vocabulary, scenario envelope, replay engine, normalization helpers,
and CLI. Backend adapters and full coverage reporting arrive in later milestones.

## Layout

| Crate | Responsibility |
| --- | --- |
| `verify-core` | invariant identity, catalog and validation, verdicts, backend capability vocabulary, `ScenarioTarget` |
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
just verify-test    # cargo nextest run
just verify-lint    # clippy -D warnings, then fmt --check
just verify-fmt     # cargo fmt
just verify-deny    # cargo deny against this workspace's own lockfile
```

CI runs the same set through a dedicated `verification` routing key, so a change
confined to this directory is actually built and tested rather than skipped.

Replay with `--catalog` validates schema and invariant identity, requires the
scenario project/model/id to match that catalog, and takes its kind from the
catalog. Search-backend binding validation is a separate `catalog validate`
operation. An unavailable or erroneous target check is reported as malformed,
never as an invariant that holds. Empty traces evaluate their initial state.

For corpus insertion, use `verify_runner::normalize` to guard syntactic rewrites
with replay, minimize reproducing violations, quarantine nondeterministic traces,
and refresh the fingerprint. The lower-level `normalize_syntactic` helper alone
cannot establish that identifier-shaped strings are safe to rename.

Identifier renaming in that runner is disabled unless the target explicitly
implements `allows_identifier_renaming() -> true`: this requires every matching
string value to be an arbitrary identifier and no references to live in object
keys. Commuting-step reordering remains independently opt-in via `commutes`.
