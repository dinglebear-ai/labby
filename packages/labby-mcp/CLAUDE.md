# labby-mcp npm Package Instructions

This package is the npm launcher/distribution wrapper for Labby. The Rust repository root remains the product source of truth.

## README Contract

`scripts/sync-readme.js` intentionally copies the repository root `README.md` into this package before packing, and `scripts/check-package.js` verifies the copies match. Do not independently edit the package README to fix a documentation discrepancy; fix the root README and resync.

## Rules

- Keep platform/binary selection deterministic and fail with actionable unsupported-platform errors.
- Do not silently download or execute an unexpected binary.
- Keep package metadata and `server.json` release expectations synchronized with the Rust release artifacts.
- Package scripts must remain usable in clean npm pack/install environments.

## Verification

Run the package's existing validation scripts after changes, including README sync/check and package tests.
