# Repository scripts

- `ci/` contains CI checks, release helpers, and their focused Python tests.
- `tests/` contains script fixtures and smoke tests. The Code Mode smoke scripts require a running gateway or build dependencies; they are run explicitly, not by the lifecycle check.
- The top level contains build, install, host setup, and developer check entry points used by the Justfile and documentation.

Run `scripts/ci/check-lifecycle-scripts.sh` after changing a shell or PowerShell script. Add new entry points to `scripts/ci/lifecycle-scripts.json` so CI checks their syntax and expected test behavior.
