---
title: "Bounded Skill Discovery - Outstanding Work"
created: 2026-09-26
updated: 2026-09-26
---

# Remaining handoff work

1. Create and qualify a Linux Labby candidate in a separate staging runtime against the real upstream fleet. Record cold/warm Code Mode no-op and scoped timings, real Depot catalog behavior, zsh async/PTY/cancel, failure/soak and rollback evidence. Production must not be replaced before the complete matrix passes.
2. Complete the comprehensive large-catalog discovery design. This patch bounds smaller provider requests and removes out-of-scope work; it does not make the 71k Depot inventory exhaustive or change existing per-upstream safety caps. The legacy unscoped canonical aggregate is still a separate architectural follow-up.
3. Finish Depot repository ingestion. Existing dinglebear-ai/depot PR #112 is open at 8436e81, with failing release lifecycle sentinel, PR live E2E shard 1 and dependency audit checks. No Depot source or service was modified here. CLI log-failed downloads returned empty logs, so the three root causes are not yet established.
4. Verify/provision the named private Git credential file using an authorized administrative path, then register limetech-marketplace and prove recursive discovery, unchanged refresh, withdrawal and Code Mode read/get. The unprivileged labby account could not read the credential environment through sudo; this is not proof that the setting is currently absent.
5. Qualify the Microsandbox runtime/MCP pair without bypassing the launch-contract check, then complete the Ubuntu 24 engineering image, projected secrets, tailnet connection, Git credentials, Cortex source receipts and warm snapshot by digest.
6. Finish device-scoped zsh installation, Labby/Dendrite operating Skills and bounded preflight/network snippets.

## Required issue created

Cortex Microsandbox execution/lifecycle/runtime telemetry: https://github.com/dinglebear-ai/cortex/issues/256 .

## Local validation caveats

The macOS linker emits a nonfatal compact-unwind size warning. The local checkout has no prebuilt gateway-admin web bundle and the build warns about empty embedded web assets. These local test binaries are not a production-ready Linux/web deployment artifact. No new tests were ignored to obtain passing results.
