---
name: implement-in-microsandbox
description: Implement a repository task in a persistent Microsandbox, preserve research and test evidence, publish an exact Git commit through an authorized path, and retain a separate verified staging instance without touching production.
---

# Implement in Microsandbox

Use this workflow for an explicitly authorized implementation task. It is a
repository-independent process, not a new Labby deployment service. Execution
requires a running Labby connection and a usable Microsandbox host. This skill
never installs or updates the host runtime, rewrites gateway configuration, or
implicitly authorizes publication, merging, production changes, or messaging.

## Non-negotiable boundaries

- Use a named, persistent development microVM and a Git branch, not worktrees.
- Read the dispatched repository's instructions and preserve existing changes.
  Do not switch or reset somebody else's host checkout.
- Keep development, staging, and production separate. Bind staging to loopback
  by default. Do not mount production data, credentials, sockets, or SSH keys.
- Do not use Claude Agents, agent workflows, or exGPT in this workflow. Ordinary
  Claude MCP file/shell tools are allowed. Parallel lanes mean independent,
  explicitly scoped operations, not permission to launch an excluded agent.
- Permission to commit is not permission to push, merge, or deploy. Never push
  to any repository owned by the Unraid organization without explicit permission
  in the current conversation. Inspect the actual destination, not its alias.
- Preserve failed evidence. Never describe an HTTP fixture as an application
  deployment, a dry run as a push, or a local test as an external live smoke.

## 1. Recover context and establish preflight gates

Retrieve the original task and last execution evidence. Record the objective,
acceptance criteria, repository owner/name, source ref, exclusions, and the
operations authorized for this run. Search the current Labby catalog for the
needed skills and tools. Inspect their exact live schemas before calling them.
Prefer Code Mode for dependent calls and compact result processing. Use
codemode.batch for independent gateway operations, not Promise.all.

Read repository and nearest directory instructions. Determine current source,
branch, dirty state, toolchain, test commands, configuration, startup path,
health endpoint, ports, storage, and real-service dependencies. Research current
official documentation where behavior is version-dependent. Record sources,
versions, checksums, decisions, counterexamples, and unresolved questions in
RESEARCH.md. Do not infer implementation from an earlier assistant's summary.

Before implementation, prove all of the following:

1. The selected runtime and MCP adapter can list, inspect, create/start, execute,
   transfer files, inspect logs, and stop/resume the intended task sandbox.
2. The development image has the required tools; the working directory exists;
   CPU, memory, disk capacity, ports, and retention are explicit.
3. The repository can be read and the intended branch can be published through
   an authorized route. A read-only Git operation is not a push-auth check.
4. The proposed staging target is not production and does not reuse its state.

Do not treat an adapter error as a broken runtime. Diagnose both independently.
An explicit native-CLI route may be selected only after checking the installed
version and current help and proving it preserves the intended sandbox policy.
Record the adapter failure separately. Do not silently downgrade security,
change backend, install a runtime, or claim the failed MCP operation passed.

## 2. Establish the persistent development workspace

Discover or create a task-owned microVM with an immutable image digest. Record
its exact name, runtime version, full configuration, labels, and retention.
Create the workspace directory before configuring it as the default workdir.
Clone the dispatched repository inside the microVM. Preserve existing branches;
create a task branch from the verified base SHA. Keep the workspace between
implementation waves. Check capacity before large dependency/build operations.

Use a project-specific, locked development image. The included Ubuntu ARM64
package lock and [bootstrap script](scripts/bootstrap_ubuntu.sh) are a tested
reference, not a universal project toolchain. They pin the complete observed
package set and fail if a version cannot be obtained. They do not promise an
archived package mirror or bit-for-bit rebuilds. For long-lived reproducibility,
publish the prepared image by digest and preserve package sources/provenance.
Do not silently substitute latest versions or install compilers in staging when
a verified release artifact is available.

## 3. Keep a durable task record

Under docs/tasks/<task-slug>/, or the repository's mandated equivalent, maintain:

- TASKS.md: acceptance criteria, task ownership, dependencies, and status.
- RESEARCH.md: source-backed findings and rejected alternatives.
- IMPLEMENTATION_PLAN.md: bounded waves, gates, rollback, parallel lanes.
- PROGRESS.md: actual operations, outcomes, current state, next action.
- ISSUES.md: failures, blockers, severity, root causes, resolution evidence.
- CHANGELOG.md: behavior, configuration, and documentation changes.
- REFLECTION.md: review findings, remaining risks, and workflow improvements.

Store raw command receipts separately from derived reports. Each receipt needs
target, command/schema, timestamp, exit status, source SHA, and output location.
Exclude secrets, not useful hostnames, paths, tool names, or diagnostics.
Preserve append-only external release evidence instead of rewriting the source
commit to insert its own SHA. A JSON claim is not proof that its command ran.

## 4. Implement in gated waves

Write a failing behavior/regression test first. Implement the smallest coherent
change, rerun focused tests, then inspect the diff and failure paths. Repeat for
subsequent waves. Verify cancellation, bounded timeouts, error handling,
retry/backoff, idempotency, resource cleanup, and observability when relevant.
Do not add generic machinery that the requested behavior does not need.

Independent research and read-only checks may run in parallel. Sequence shared
workspace edits, migrations, dependent tool calls, Git operations, and promotion.
Use bounded batches and inspect partial effects before retrying a failed call.

Checkpoint only after recording the workspace/configuration. Stop the sandbox
when required by the installed snapshot contract. Record and verify the exact
returned snapshot digest. Do not assume name/path/digest lookup is equivalent.
Resume and re-inspect the original development sandbox. Checkpoints are recovery
inputs, not releases: they do not prove configuration or running-process state
was retained, and they must not be the staging promotion artifact.

## 5. Validate, review, and publish

Run focused tests, repository-required gates, documentation/link/configuration
checks, and appropriate real-service smoke tests. Capture failures and fix them.
Review changes against requirements, architecture, security boundaries, and
existing patterns. Use allowed review tools only. A self-review is not an
independent review; report which happened. Never fabricate a green CI result.

Inspect the exact diff, commit only task-owned changes, and verify a clean tree.
Use one of two explicitly recorded publication modes:

- Projected credentials: use the current Microsandbox secret-reference schema,
  narrowly scoped to the destination host. Never send literal credentials in
  tool parameters, environment values, logs, snapshots, or Git URLs.
- Host SSH broker: export a Git bundle from the sandbox, verify its head, import
  it into a new isolated bare repository, and use the host's existing authorized
  SSH identity. Do not copy or forward the key into the sandbox. Check the actual
  owner/repository and destination branch, dry-run, publish that exact SHA, then
  read the remote ref back. A broker is not proof of in-VM Git authentication.

On the validated msb 0.7.3 CLI, protected secrets use host environment references
such as --secret GITHUB_TOKEN@github.com. Inline NAME=VALUE is rejected. Inspect
current help rather than copying older skill examples.

Create a PR only when authorized. Include scope, tests, review, known failures,
links to evidence, and production impact. Leave it unmerged unless merging was
explicitly requested. Reconcile changed upstream code rather than force-pushing
or dropping another contributor's work.

## 6. Build and retain staging from the exact release

Build from the clean, published source SHA using locked inputs. Record the
artifact SHA-256, runtime image digest, build command, and toolchain. Copy only
the verified release/configuration into a newly named staging microVM. Do not
promote the development snapshot or mount the development checkout.

Declare CPU, memory, working directory, command, port mapping, labels, secrets by
name, and retention. For a request to leave staging running, use persistent
state and explicitly record no maximum-duration or idle-expiry cutoff. Inspect
the actual resulting configuration; requested settings are not evidence.

[verify_handoff.py](scripts/verify_handoff.py) implements a deliberately narrow
reference gate: no host mounts or environment values, loopback HTTP, immutable
image/source/artifact identity, explicit retention, and test/doc receipts. Use
[the manifest contract](references/handoff-contract.md). Extending that policy
requires an explicit review, not removing failing checks.

Run the verifier against the artifact before launch. Then, from the host outside
the staging VM, run it with --probe. The health response must identify the exact
sandbox, staging environment, commit, and artifact digest; it must not redirect.
Also exercise actual application behavior, negative paths, logs, and configured
real dependencies. Test restart/recovery when it is part of acceptance criteria.
Retain staging and verify it remains healthy at handoff. Do not delete it during
cleanup, and do not modify production to make staging work.

The included [fixture server](scripts/fixture_server.py) demonstrates only this
workflow's transport and provenance checks. Its success never validates Labby's
Rust server, MCP gateway, UI, or another application's behavior.

## 7. Handoff honestly

Report the commit/PR, exact dev and staging identities, access scope, test and
review evidence, runtime configuration, artifact hash, current health, retained
state, and unresolved blockers. Distinguish attempted, locally passed,
published, CI-passed, staged, and production-deployed. Link the complete record.
Send a completion notification through Gotify only when the task authorized it;
do not send a completion claim while required gates are still blocked.

### Reference validation command

Run from this skill directory:

    python3 -m unittest discover -s tests -v

The fixture verifier uses Python 3.11 or later and no third-party dependencies.
See [the handoff contract](references/handoff-contract.md) for executable usage.
