# Standalone installer bootstrap trust

This note defines the reviewed bootstrap boundary. It does not claim OS signing, notarization, or a clean-machine release result.

## Initial trust

The user obtains and executes Labby's canonical HTTPS installer. Trust in that initial script comes from its canonical delivery origin and the user's choice to execute it; a script cannot independently prove its own authenticity before execution. This is the existing initial installer trust boundary. Operators who require independently verified installer bytes can continue verifying its published attestation before execution with a separately trusted verifier.

A reviewed immutable verifier URL and SHA-256 embedded in that trusted script can bootstrap verification without requiring a GitHub account or preinstalled GitHub CLI. The expected digest must be part of the reviewed installer source, not fetched alongside the verifier at runtime. Changes to verifier version and pins require code review and an attested installer release.

## Verified pin inventory

`scripts/ci/github-verifier-bootstrap-pins.json` records GitHub CLI `2.102.0` platform archives and immutable release metadata. The official [release API](https://api.github.com/repos/cli/cli/releases/tags/v2.102.0) reports `immutable: true`. The downloaded [checksum manifest](https://github.com/cli/cli/releases/download/v2.102.0/gh_2.102.0_checksums.txt) matches its API digest, and each selected archive checksum matches the API asset digest.

The [official security advisories](https://github.com/cli/cli/security/advisories) identify source-ref case matching, signer-workflow prefix matching, and filesystem symlink issues affecting versions through `2.101.0`; `2.102.0` is the patched release. An existing older verifier must not satisfy the installer prerequisite merely because it supports `attestation verify`.

## Required bootstrap behavior

1. Resolve a supported platform to an exact reviewed archive URL and embedded digest. No `latest` lookup and no runtime replacement checksum.
2. Download to an owned private temporary directory, hash the complete archive, and reject a mismatch before extracting or executing any bytes.
3. Extract only the expected regular verifier executable into that private directory. Reject archive traversal, links, unexpected layout, or absent binaries. Do not overwrite an installed GitHub CLI or write into a shared executable cache.
4. Use the verified executable for Labby's existing provenance policy: canonical GitHub host, requested repository, exact release signer workflow and tag ref, hosted runners, and artifact digest. Public local bundles support account-free verification.
5. Preserve immutable activation receipts, rollback, and interrupted-install recovery. Clean the private bootstrap directory on exit.

The pins describe verifier bootstrap bytes, not an alternative trust policy for Labby releases. They do not authorize a privileged catalog token, unrestricted source fallback, arbitrary README execution, or weaker catalog access checks.

GitHub documents bundle and custom trusted-root inputs in the [verifier reference](https://cli.github.com/manual/gh_attestation_verify). Root material is a separate concern from authenticating the verifier executable itself. OS-signed installers remain a possible stronger initial distribution channel, but are not required to describe this explicit HTTPS-script bootstrap boundary honestly.

## Source implementation

The [shared generator](../../scripts/ci/generate-verifier-bootstrap.py) embeds
one reviewed inventory into [the Unix installer](../../scripts/install.sh),
[the Windows installer](../../scripts/install.ps1), and the reusable CI shell
helper. Unix uses bounded HTTPS downloads and exact archive-member extraction.
Windows uses bounded HTTPS redirects and streaming downloads, a protected
private directory, hash-before-extraction, and only the regular `bin/gh.exe`
member. Neither changes PATH or replaces the user's installed verifier.

Both installers accept a fixed stable verifier version of at least `2.102.0`.
Public bundle verification removes all four GitHub token variables, pins
`GH_HOST=github.com`, and uses an empty private configuration directory. Windows
restores its previous process environment in `finally`. Only a missing sidecar
(HTTP 404) selects the legacy lookup requiring the user's own authentication;
malformed, rejected, or unavailable bundles never select that fallback.

CI provenance/export/observer consumers use the same minimum-version shell
helper and release identity policy. Behavioral fixtures cover missing and old
verifiers, corrupt download rejection before execution, generated trust-code
parity, and credential isolation. Windows Pester cases require a PowerShell
runner; source tests and reviewed archive layouts alone do not prove Windows
execution. Current published release availability remains a separate gate.

## Qualification

The timed harness must include verifier download/hash/bootstrap in its stopwatch and record the chosen verifier version and bootstrap method. Platform fixtures must use no developer account and cold verifier caches. Contract fixtures do not establish real attestation verification, fresh-machine first use, or the five-minute promise; those require actual released artifacts and real Agent/catalog/MCP results.
