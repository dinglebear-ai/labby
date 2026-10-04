# Repository scripts

- `ci/` contains CI checks, release helpers, and their focused Python tests.
- `tests/` contains script fixtures and smoke tests. The Code Mode smoke scripts require a running gateway or build dependencies; they are run explicitly, not by the lifecycle check.
- The top level contains build, install, host setup, and developer check entry points used by the Justfile and documentation.

Run `scripts/ci/check-lifecycle-scripts.sh` after changing a shell or PowerShell script. Add new entry points to `scripts/ci/lifecycle-scripts.json` so CI checks their syntax and expected test behavior.

## Pinned provenance verifier

`ci/github-verifier-bootstrap-pins.json` is the reviewed trust inventory. Run
`python3 scripts/ci/generate-verifier-bootstrap.py` after an explicitly reviewed
pin change, then synchronize installer copies. The generator emits
`ci/github-verifier-bootstrap.sh` and the embedded Unix and Windows installer
blocks from the corresponding `.sh.in` and `.ps1.in` templates. Both templates
consume the same reviewed pin inventory. The helper requires GitHub CLI 2.102.0 or newer; if absent or
older, it verifies the pinned official archive before executing its member.
CI provenance verification and bundle export use this same policy.
The caller owns a private temporary directory and its cleanup; no PATH mutation
or globally installed helper is required. Bundle verification uses an empty
credential store and clears token variables.

The Windows installer prepares a protected temporary directory, selects a fixed
verifier version, checks the embedded archive hash, and extracts only
`bin/gh.exe`. HTTPS redirects, transfer size, and transfer time are bounded.
Public release bundles are verified with token variables cleared and an empty
configuration directory; the previous process environment is restored afterward.
Only a missing bundle (HTTP 404) uses the authenticated legacy lookup. Invalid
bundles never fall back. Current release availability and Windows runtime
qualification remain separate from this source implementation.
