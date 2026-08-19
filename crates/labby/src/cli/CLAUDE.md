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
