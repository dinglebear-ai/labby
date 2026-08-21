# @dinglebear/labby — npm Launcher Package

This package is a thin distribution launcher for the Labby binary. It is not a JavaScript reimplementation of Labby.

## Release Contract

- `README.md` is intentionally synchronized byte-for-byte from the repository root by `scripts/sync-readme.js`; do not edit the package copy independently
- license files are synchronized from the repo root
- postinstall downloads a platform release artifact and verifies SHA-256 before use
- runtime scripts must not depend on files outside the published package
- `server.json`, package version, npm metadata, and release artifacts must stay aligned

Run the package checker/tests after launcher or packaging changes. Keep the published tarball minimal.
