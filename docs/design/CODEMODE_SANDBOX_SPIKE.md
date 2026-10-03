---
title: "Code Mode sandbox spike"
created: "2026-10-02"
updated: "2026-10-02"
---

# Code Mode sandbox spike

This experiment adds `codemode.sandbox.run(request)` using the local Microsandbox
Rust SDK, pinned to 0.7.3. It is separate from the existing MSB transport that
isolates the JavaScript runner. Here JavaScript asks the parent broker to launch
a disposable guest workload and receives a structured result.

## Enable

Build Labby with `--features codemode-microsandbox`. The kernel feature is
`microsandbox-sdk`. Neither is enabled by default. Install the compatible MSB
runtime and prepare the image cache separately; the adapter never installs a
runtime or pulls an image.

The operator supplies `LABBY_CODE_MODE_SANDBOX_PROFILE_JSON`:

```json
{
  "image": "registry/image@sha256:<64 hexadecimal characters>",
  "cpus": 1,
  "memory_mib": 256,
  "timeout_ms": 15000
}
```

The image must be immutable and cached. Requests must match it exactly. Limits
are 1–2 CPUs, 128–1024 MiB RAM, and at most 15 seconds of workload execution,
including file projection. Startup has a separate 15-second budget and cleanup
has a 5-second budget. Two guests can be admitted concurrently.

## Request

```javascript
async () => {
  return await codemode.sandbox.run({
    image: "registry/image@sha256:<64 hexadecimal characters>",
    command: ["node", "/work/analyze.js"],
    timeout_ms: 1000,
    files: { "analyze.js": "console.log(JSON.stringify({answer: 42}))" }
  });
}
```

The command is an executable and arguments, without an implicit shell. Inline
text files are projected under `/work`; at most 16 flat filenames and 128 KiB
combined are allowed. Host paths, mounts, network configuration, and credential
fields are rejected. The guest uses restricted security, disabled networking,
UID/GID 65534, and a bounded temporary root disk. stdout and stderr together
are capped at 64 KiB.

Success returns `ok`, `exit_code`, `stdout`, `stderr`, `sandbox`, `image`,
`elapsed_ms`, `network`, `projected_files`, and `cleanup_confirmed`. Cleanup must
succeed before a successful receipt is returned. Nonzero process exit is a
receipt with `ok: false`. Timeout, cancellation, output overflow, projection,
startup, and cleanup failures use the existing Code Mode error contract.

## Authorization and lifecycle

`sandbox` is a reserved local namespace. Existing local-provider authorization
allows trusted local and unscoped admin callers; scoped or read-only callers
cannot use it. Dispatch happens in the parent, outside the state/git lock.

Dropping the invocation signals an owned worker to finish startup and destroy
its guest. Guests are named `labby-workload-<ULID>` and labeled
`owner=labby-codemode-spike`. Cleanup failure or uncertain startup quarantines
an admission slot until process restart. A 60-second guest lifetime bounds
runtime survival, but is not proof of persisted-state removal.

## Verification

Ordinary focused tests:

```sh
cargo test -p labby-codemode --features microsandbox-sdk sandbox
```

The ignored `sandbox_live_codemode_projection` and
`sandbox_live_timeout_cleans_guest` tests require the operator profile and a
cached Node image. The projection test also requires
`LABBY_CODE_MODE_RUNNER_EXE` pointing to a compatible Labby executable, and
exercises JavaScript, framed local dispatch, SDK projection, and guest execution.
Inspect the owned label afterward to verify guest removal.

## Remaining production work

This is an execution spike, not published-tool support. It does not implement
named profile configuration, guest-to-Labby credential brokering, secret
projection, output-file export, or snippet publication metadata. Caller
cancellation, crash recovery, and startup races need broader qualification and
an ownership reconciler before production use. The SDK adds a substantial
optional dependency graph; evaluate binary size and build cost before adopting
it in default product features. Full workspace feature and CI gates remain
separate from focused kernel verification.

## Spike observations

A local macOS ARM64 smoke run against MSB 0.7.3 passed all four live tests:
projection through the actual JavaScript runner, workload timeout, invocation
drop cancellation, and output overflow. The projected program returned `answer: 42`, UID 65534, only the
`lo` interface, and no GitHub token in its environment. The successful receipt
confirmed cleanup. A subsequent CLI inventory contained no `labby-workload-`
guests. Cancellation signaling on invocation drop also has a focused unit test;
process crash and startup-race recovery remain unqualified.

The CLI image inventory can list a digest derived from a cached tag without
having the SDK's digest-specific cache metadata. Explicitly preparing the
immutable reference with `msb image pull <reference> --materialize layered`
resolved that startup failure. This preparation is an operator step, outside
request execution; `PullPolicy::Never` remains enforced in the adapter.

Kernel verification passed: 370 unit tests plus 2 error-schema integration tests;
4 live tests were run separately and passed. Strict Clippy passed with the SDK
feature and without default features. These results do not establish full
workspace feature-slice, CI, release, or deployment readiness. Application compilation passed with the
new feature, both with defaults and as a standalone feature slice. The new slice
is included in `scripts/check-feature-slices.sh`; the full slice matrix has not
been run for this spike.
