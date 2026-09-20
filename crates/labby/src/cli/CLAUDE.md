# cli/ — clap Adapters

CLI code parses arguments, invokes shared dispatch/runtime behavior, and renders the result. It does not own product operation semantics.

## Rules

- keep command adapters thin; move shared validation/operation logic into dispatch or the owning reusable crate
- use the shared output system under `crate::output`; do not add command-local styling frameworks
- `--json` must remain machine-readable and unstyled
- destructive confirmation derives from shared action metadata plus CLI policy
- admin authorization and destructive classification are separate concepts
- preserve typed errors; use `anyhow` only at the outer CLI boundary where appropriate
- use current `labby ...` command examples, never the retired `lab ...` binary name

The generated `docs/generated/cli-help.md` file is the authoritative command/flag snapshot.

## Public command contracts

- Public command and alias names contain no hyphens. Use resource/action words, not concatenated substitutes. Flags and resource IDs may contain hyphens.
- Canonical operator groups lower through `Command::into_operation()` to existing typed handlers. Internal skipped variants are not executable compatibility aliases. Do not duplicate backend authorization or mutation policy.
- `set` patches fields; `replace` retains complete-replacement semantics. Keep local versus daemon-backed authority explicit.
- Help, JSON command inventory, generated docs, and completion derive from the Clap graph. Do not reintroduce the MCP service/action catalog as root CLI help.
- Errors must be rendered independently of tracing and preserve the shared kind, origin, effects, and recovery contract. Never log argv, credentials, source code, or parameter payloads. Correlate operations with request IDs and elapsed times.
- Dry-run previews are redacted JSON under `--json`; previews never execute. Once remote Code Mode execution is attempted, do not retry through a local fallback.
- Run `cargo test -p labby --lib cli::`, `cargo test -p labby --lib entrypoint::`, and `cargo test -p labby --test cli_contract`. Regenerate/check all-feature docs and verify feature-gated builds before release.
