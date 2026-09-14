# Labby Product Qualification Contract

Status: planned acceptance criteria, not a claim of implemented coverage.
Execution owner: Beads epic `lab-jfu6q`. Reuse the real-process infrastructure
from completed epic `lab-7km4s`; do not replace it with a second harness.

The completed Q0 source inventory is [INVENTORY.md](INVENTORY.md). Its status
labels describe evidence present in this checkout, not completion of Q1-Q6.

## Common Acceptance Rules

Q0 inventories current tests, public contracts, and fixtures before Q1–Q6.
For each row record its owning test, transport, positive/negative paths, current
support status, evidence lane, and remaining gap. An existing unit test is not
automatically E2E. Unsupported behavior gets a tested rejection and an explicit
exclusion; implementing a new product capability requires separate scope.

Real-process tests launch the compiled product with isolated state and local
fixture upstreams. Assert public results, relevant persisted state and upstream
effect counters, typed errors, secret redaction, and process/session cleanup.
Control races with acknowledgements/barriers. Record source and binary identity,
fixture versions, auth subject (never credentials), seed, limits, and artifacts.
Keep literal expected results independent of the production function under test.

## Coverage Matrix

| Milestone | Surface | Required success and failure coverage |
| --- | --- | --- |
| Q1 | MCP tools | Initialize/negotiate, discover, call through service action/params and gateway tools; schema validation, policy denial, unknown entries, structured errors |
| Q1 | Resources and prompts | List/get/read, text/blob, templates and argument completion where supported; bad arguments/URIs, ownership collisions, pagination/cursor bounds, oversize responses |
| Q1 | Skills over MCP | Discovery and retrieval with provenance, federation/proxy behavior, disabled capability and trust/subject isolation; do not assume a nonstandard method exists |
| Q1 | Elicitation | Accepted, declined, cancelled, malformed, timeout and disconnect; correlation and destructive policy preserved; unsupported client fails according to contract |
| Q1 | Protocol lifecycle | Supported transports/versions, cancellation, progress, tasks, subscriptions/list changes, sampling and roots where implemented; explicit unsupported contracts otherwise |
| Q2 | Bearer auth | Valid, absent, malformed, expired/revoked credentials; read/admin/destructive gates, subject isolation, no credential leakage |
| Q2 | Google and Authelia OAuth | Login/callback, state/PKCE, discovery, token exchange/refresh/revocation, restart and failure paths as supported; deterministic provider fixtures plus separately credential-gated real-provider qualification |
| Q2 | GitHub OAuth support decision | Verify inbound and upstream support separately against live provider registration/configuration; report unsupported honestly, do not infer OAuth support from Depot GitHub ingestion or add a provider in a test task |
| Q3 | Code Mode | Discovery/describe/execute; fanout, dependent calls consuming actual earlier results, partial errors, bounded stress, timeout/cancel, output/memory/fuel/queue limits as implemented; no duplicate or post-fence effects |
| Q3 | Code Mode primitives | Prompt/resource/template retrieval as inert data, capability revocation, stale catalog generations, hidden entries, scope isolation, URI ownership, native UI metadata |
| Q4 | WebMCP bridge | Real browser page and installed bridge path through the real gateway; discovery/invocation/results, reconnect, origin/subject denial, cancellation, tab/extension teardown; socket simulator evidence reported separately |
| Q4 | MCP Apps | Resource MIME and UI metadata, render, tool callback/result, policy and reauth, CSP/origin enforcement, disconnect and teardown; OpenAI and Anthropic host behavior separately version-pinned and capability-detected |
| Q5 | Proxying | Tools, prompts, resources/templates, skills, elicitation, and MCP Apps each exercised client → Labby → fixture upstream, including a multi-hop case; preserve metadata/correlation and prevent namespace/subject collisions |
| Q5 | Proxy failures | Upstream auth and disconnect, reconnect, slow/oversize/malformed replies, cancellation, catalog invalidation, partial failure and secret redaction; verify no double execution |
| Q6 | Send to Labby | First resolve the canonical user-facing delivery entry point and receipt contract in Q0. Then test real send → acceptance → persisted/retrievable result, authority, exact revision, duplicate/retry, invalid payload and interruption; do not substitute a guessed feature |
| Q6 | Depot ingestion | GitHub, skills.sh, marketplace.json and plugin.json: source resolution → ingest → persisted catalog → Labby discovery/use; provenance/revision pins, nested paths, pagination, malformed manifests, duplicates, unavailable/rate-limited source, traversal/SSRF/trust rejection, partial-ingestion recovery |

Cross-cutting acceptance includes startup/shutdown and restart persistence,
expired authority between discovery and execution, multi-subject isolation,
resource leaks, platform/feature slicing, and release-binary smoke coverage.
Use supported-platform cases, not impossible transport requirements on every OS.

## Evidence and Scheduling

| Lane | Schedule and initial execution cap | Evidence required |
| --- | --- | --- |
| Model/replay | SPEC T0–T3 | Model identity, invariant, bound/seed, replay status; never product E2E credit |
| Lifecycle conformance | C1, PR; 5 minutes excluding build | Same controlled trace, observation relation, real binary, negative adapter self-test |
| Deterministic process/browser | PR; 15 minutes per job excluding build | Required matrix rows, real boundary assertions, fixture identities, cleanup |
| Stress/extended matrix | Nightly; 45 minutes per job | Bounded workload/concurrency, seed, latency/error counts, leak/cleanup checks |
| Release qualification | Release candidate; 60 minutes per job excluding build | Exact release binary and required transport/platform matrix |
| Actual hosts/providers | Credential-gated/manual; 30 minutes per journey | Host/provider version, dated interaction evidence, render/callback or OAuth lifecycle, cleanup |

Configure finite build and teardown timeouts separately. Establish resource and
latency baselines in Q0/Q3 before adding performance regression thresholds.
Timeouts, missing credentials, unavailable hosts, and excluded rows must remain
visible. Required rows cannot be waived by a skipped job or emulator success.
Deterministic host emulators run in CI; actual OpenAI/Anthropic qualification is
separate and must not be advertised as covered by Node/fake-DOM tests. Record
host-specific unsupported behavior instead of assuming identical capabilities.

## Ownership and Ordering

- Q0 can begin alongside M0; Q1–Q6 follow its inventory and fixture contract.
- C1 follows M3/M4 and is required before claiming model-to-product assurance.
- Q1 reuses `lab-jpo9u.11` for native client lifecycle/security/transport work.
- Q3 reuses `lab-ykxu5.7` for Code Mode primitive E2E and its prerequisite graph.
  The new qualification task owns fanout, dependent-call and stress gaps, not
  a competing implementation of those existing primitives.
- Q4/Q5 can build on existing fixtures while Q1 completes; final qualification
  requires their shared protocol contracts to pass.
- Q6 crosses into Depot only after verifying the correct checkout and owner.
  Record both repository revisions and hand off Depot changes in its own
  tracked work; this Labby plan does not authorize editing another repository.
- An unresolved Send to Labby contract blocks only that row, not foundational
  implementation or the rest of the E2E matrix.

Do not reopen `lab-7km4s` or equate its completion with this expanded matrix.
No production deployment or actual-host account mutation is implied by planning.
