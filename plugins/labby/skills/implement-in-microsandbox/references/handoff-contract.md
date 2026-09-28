# Staging receipt and verification contract

This is a restricted reference workflow, not a general deployment API. The
verifier checks consistency, artifact bytes, and optionally live HTTP identity.
It cannot attest that user-entered tests ran, that a published_commit field was
observed on GitHub, or that a health endpoint truthfully describes application
bytes. Preserve and review raw build/test/Git/transfer/configuration receipts.

## Required receipt fields

| Field | Required meaning |
| --- | --- |
| schema_version | Integer 1 |
| environment | staging only |
| source | repository owner/name, branch, complete commit and observed published_commit, dirty=false |
| artifact | kind=workflow-fixture or application, lowercase 64-character sha256 |
| sandbox | distinct name/development_name, image with @sha256 digest, positive cpus/memory_mib, absolute workdir, command argument list |
| sandbox.ports | Explicit loopback 127.0.0.1 host_bind plus host_port and guest_port |
| sandbox.persistent | true |
| sandbox.max_duration_secs | Explicit null, not an omitted field |
| sandbox.idle_timeout_secs | Explicit null, not an omitted field |
| sandbox.mounts / env | Empty array / empty object for this narrow reference |
| sandbox.secret_names | Names only; values never belong in the receipt |
| checks | Named passing tests and docs entries, executable argument lists, positive tests count |
| health_url | http://127.0.0.1:<mapped-port>/healthz with no credentials or redirects |

A snapshot field is intentionally rejected. Use a separate checkpoint record.

## Commands

    python3 scripts/verify_handoff.py deployment.json --artifact release.tar

After starting staging, execute from the host outside that VM:

    python3 scripts/verify_handoff.py deployment.json --artifact release.tar --probe

Exit status 0 means this reference gate passed; 1 means a rejected receipt,
artifact, or live identity; 2 means unreadable/malformed input JSON. It does not
mean the application, CI, security review, or production deployment passed.
Raw output is JSON suitable for a bounded operation receipt.

The fixture server serves only / and /healthz. It does not expose directory
listings or an arbitrary-file route. Its homepage explicitly says it is not a
Labby application. Health includes environment, sandbox, commit,
artifact_sha256, and kind. Logs are structured JSON on stdout/stderr.

## Bootstrap reference

The checked-in complete package lock was captured from Ubuntu 26.04 Linux ARM64
with Git 2.53.0, Python 3.14.3, and the image manifest below. It is architecture
specific. Inspect architecture and actual image before using it.

    docker.io/library/ubuntu@sha256:a6757b311b671e9379ac0548b5abf7a7e68d33bff51f6044bbd60f68f781489a

    sh scripts/bootstrap_ubuntu.sh references/ubuntu-arm64.packages.lock

The script creates /workspace, checks all locked packages, installs exact
missing versions through apt's authenticated repositories, and verifies them
again. It is repeatable on the validated base. Repository retention is not
guaranteed: unavailable versions must fail rather than float. A distributable
prepared image should be built once and published by immutable digest.

## Current external references

- [Official documentation index](https://docs.microsandbox.dev/llms.txt)
- [Official Rust sandbox lifecycle/configuration](https://docs.microsandbox.dev/sdk/rust/sandbox)
- [Official image/digest behavior](https://docs.microsandbox.dev/images/disk-images)

Installed runtime help and live tool schemas must be checked alongside these
references. The September 26 validation found a stale MCP launch adapter even
though native msb 0.7.3 could start the same sandbox; adapter and runtime results
are separate evidence. No compatibility guard was removed or runtime replaced.
