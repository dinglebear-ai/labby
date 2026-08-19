# Repository Scripts Instructions

`scripts/` contains repository automation, CI helpers, install/host-service tooling, and safety checks.

## Rules

- Prefer deterministic, noninteractive behavior in CI-facing scripts.
- Preserve strict error handling and actionable diagnostics. Shell scripts that mutate host state should identify the target before changing it.
- Do not bake DOOKIE-specific paths, credentials, or transient worktree locations into reusable scripts unless the script is explicitly host-specific and documented as such.
- Keep changed-path classification synchronized with CI tests when adding or removing repository surfaces.
- Retired-feature guards are policy enforcement, not historical examples; update them only when the product contract changes intentionally.
- Never print secrets or authorization values in debug output.

Validate modified scripts with their focused tests plus shell/Python syntax checks where applicable.
