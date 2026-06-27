# Backup Node Live Test Services Design

Date: 2026-04-12
Status: Proposed
Scope: opt-in live end-to-end test infrastructure for `lab`

## Summary

`lab` already distinguishes between CI-safe tests and opt-in live verification. The missing piece is a reproducible live environment that can exercise the real public surfaces of the project without pointing tests at personal homelab services.

This design establishes SSH host `backup-node` as the dedicated remote fixture host for live end-to-end testing. Test services run there as disposable Docker stacks backed by per-run ZFS clones created from prebuilt golden snapshots. Each run gets isolated service state, stable connection details, and deterministic teardown.

The goal is broad live coverage against real service processes:

- every service CLI command or subcommand that is implemented and fixture-supported
- every MCP action, including discovery actions where applicable
- every HTTP API endpoint exposed by `lab`
- expected success and failure paths with explicit result classification

This environment is the canonical automated live environment. Personal homelab services remain optional for manual operator checks only.

## Goals

- provide a reproducible live environment for broad end-to-end testing
- avoid using personal long-lived services as automation targets
- isolate each run so destructive tests can mutate state safely
- make coverage explicit at the action and endpoint level
- keep orchestration concerns out of normal service clients and dispatch code
- produce evidence of exactly what passed and failed in a given live run

## Non-Goals

- replacing CI-safe SDK, dispatch, CLI, MCP, or API tests
- embedding remote orchestration logic into `lab-apis`
- making live tests mandatory in CI
- guaranteeing that every upstream service API operation is modeled on day one
- using `extract` as the orchestration engine for the live environment

## Background

The existing repo contracts already require:

- CI-safe tests for SDK, shared dispatch, and surface adapters
- opt-in live verification against real services when available
- observability evidence for live paths
- careful handling of destructive behavior

`extract` already supports SSH-backed reads using `host:/absolute/path` URIs. That capability is useful for bootstrap and inspection, but it should remain separate from live test fixture orchestration.

## Chosen Approach

Use `backup-node` as a dedicated remote fixture host. For each live test run:

1. create a unique run ID
2. clone service datasets from read-only ZFS golden snapshots
3. start Docker containers or Compose stacks against those cloned datasets
4. wait for readiness and emit a machine-readable run manifest
5. execute a catalog-driven live case matrix from the repo workspace
6. collect structured results and artifacts
7. tear down containers, network resources, and cloned datasets

This yields real end-to-end testing against live services while keeping reset cheap and deterministic.

## Alternatives Considered

### 1. Long-lived shared services on `backup-node`

Rejected.

Problems:

- state drift accumulates between runs
- destructive tests become dangerous or impossible
- failures become ambiguous because multiple runs share history
- recovery depends on manual cleanup

### 2. Docker volumes only

Rejected as the primary mechanism.

Problems:

- weaker reset guarantees than ZFS clone and destroy
- more manual work to keep seeded state versioned and reproducible
- harder rollback when fixture state becomes inconsistent

### 3. API-seeded empty services for every run

Rejected as the primary mechanism.

Problems:

- too slow for broad service coverage
- too much setup logic must be maintained in tests
- some services need realistic preconfigured state to make coverage useful

## Architecture

The system has five major units.

### 1. Fixture Definitions

Declarative service fixture definitions describe:

- service name
- Docker image or Compose service name
- required ports
- readiness checks
- auth details to expose to the test harness
- dataset names and snapshot identifiers
- supported live case capabilities
- whether destructive live mutation is supported

These definitions should be small and versioned in the repo.

### 2. Host Orchestrator on `backup-node`

The host orchestrator owns:

- cloning ZFS datasets from golden snapshots
- creating per-run Docker networks and volumes where needed
- starting the requested service profile
- waiting for service readiness
- returning a run manifest
- stopping services and destroying all per-run resources

This logic belongs in test infrastructure, not in `lab-apis` or normal runtime dispatch.

### 3. Repo Live Runner

The repo-side live runner owns:

- selecting a fixture profile
- establishing the SSH control connection to `backup-node`
- invoking the host orchestrator
- exporting run credentials and URLs into the test process
- launching live CLI, MCP, and API test passes
- collecting and storing artifacts

This runner should be invokable from `just` targets and direct commands.

### 4. Catalog-Driven Case Generation

The live suite should not depend on one bespoke handwritten test per command or route.

Instead, the matrix is generated from existing project truth sources:

- CLI command registrations
- MCP action catalogs
- API route registrations

Each generated surface item is matched to a live case definition that states:

- expected success or failure class
- required fixture capabilities
- any known IDs or fixture references
- whether the case is destructive

### 5. Coverage and Artifact Reporting

Every run produces structured artifacts that answer:

- what run was executed
- what services were started
- what surface items were attempted
- what each item returned
- what was skipped and why
- what observability evidence was captured

The output should support both machine-readable processing and human review.

## Ownership Boundaries

### In Scope for Test Infrastructure

- remote orchestration over SSH
- ZFS clone and destroy lifecycle
- Docker or Compose startup on `backup-node`
- fixture manifests
- readiness checks
- live result collection
- artifact generation

### Out of Scope for Product Runtime Code

- hardcoding `backup-node` knowledge into normal service clients
- embedding fixture lifecycle in CLI, MCP, or API dispatch
- teaching `extract` to provision remote services
- coupling production code paths to live test host assumptions

This keeps the product architecture clean while allowing rich external verification.

## Environment Model

`backup-node` is the dedicated live fixture host.

Each run:

- gets a unique run ID
- uses isolated cloned datasets
- runs on isolated container networking
- exposes per-run connection details only through the run manifest
- is disposable after completion

Golden datasets remain read-only sources. They are updated only through an intentional fixture refresh workflow, not during ordinary test execution.

## Data Model

### Golden Datasets

Each supported service has a preconfigured golden dataset on the ZFS pool. The dataset contains the realistic service state needed for useful testing, for example:

- known auth material
- known objects or entities
- seeded configuration
- any data needed for stable read-only and destructive cases

Golden datasets are snapshotted and treated as versioned fixtures.

### Per-Run Clones

For each test run, the host creates writable clones from the golden snapshots. Containers mount the clones as their persistent data roots. At teardown, the clones are destroyed.

This provides realistic persistence during a test while preserving deterministic reset after a test.

## Profiles

The system should support running subsets of the full environment by profile.

Examples:

- `servarr-core`
- `downloads`
- `documents-notes`
- `network`
- `ai`
- `all`

The `all` profile composes smaller profiles rather than defining a separate one-off environment. This keeps failures attributable and simplifies incremental onboarding.

## Test Scope

The live environment is the canonical automated live environment. Broad coverage is the goal.

For each onboarded service, live coverage should attempt:

- every implemented CLI command or subcommand that is fixture-supported
- every implemented MCP action, including `help` and `schema` when those are part of the public surface
- every implemented HTTP API endpoint exposed by `lab`

Each case records an explicit result classification:

- `pass`
- `fail`
- `unsupported_in_fixture`
- `skipped_destructive`
- `not_implemented`

This avoids vague “smoke tested” claims and produces precise live status.

## Live Case Model

The unit of coverage is a live case.

A live case includes:

- `surface`: `cli`, `mcp`, or `api`
- `service`
- `operation`: action name, command identifier, or route identifier
- `fixture_requirements`
- `expected_result_class`
- `expected_error_kind` or expected HTTP status where relevant
- `destructive`: boolean
- `notes`

Examples:

- `cli / radarr / system.status / success`
- `mcp / radarr / movie.get missing-id / expected unknown or not-found failure`
- `api / POST /radarr system.status / success`

## Destructive Behavior

Destructive live tests are allowed only when the fixture definition explicitly marks the service as safe to mutate under a cloned dataset.

Rules:

- destructive tests are opt-in
- destructive tests run only against writable per-run clones
- teardown must not depend on the destructive path having succeeded
- destructive failures must still preserve cleanup

This aligns with the repo testing contract while taking advantage of disposable state.

## Observability Requirements

Live runs must collect enough evidence to verify the observability contract.

At minimum, live artifacts should preserve:

- surface
- service
- action or route
- run ID
- request or correlation identifiers where available
- elapsed time
- status or stable error kind

The live suite does not replace focused observability contract tests, but it should confirm that the real integrated path emits the expected structured signals.

## Reporting

Each run should produce:

- a run manifest with service URLs, env var references (not credential values), profile, and snapshot versions
- a structured case result file
- a summarized human-readable report
- pointers to any logs or observability captures gathered during the run

The report should make it easy to answer:

- what ran
- what passed
- what failed
- what was skipped
- what fixture version backed the run

## Commands and Operator Workflow

The exact command names can be chosen during implementation, but the expected workflow is:

1. request a live environment for a profile
2. receive a run manifest
3. run the desired live matrix or subset
4. inspect artifacts
5. tear down the environment automatically or explicitly

Likely entry points:

- `just live-env-up <profile>`
- `just live-test <profile>`
- `just live-env-down <run-id>`
- `just live-test-all`

The repo should prefer machine-readable manifest exchange over ad hoc terminal parsing.

## Fixture Refresh Workflow

Golden datasets will occasionally need refresh.

That should be an explicit workflow that:

- starts from the previous golden state or from a known base image
- applies controlled configuration changes
- validates the resulting service behavior
- records the new snapshot identifier and fixture manifest

Fixture refresh is maintenance, not ordinary test execution.

## Security and Safety

- credentials emitted in run manifests must be handled as secrets
- live artifacts must avoid logging sensitive auth data
- SSH control operations must fail non-interactively
- teardown must be safe to retry
- orphaned runs should be discoverable and removable

No production or personal homelab datasets should be mounted into the live fixture environment.

## Risks

### Fixture Staleness

Golden snapshots can become inconsistent with current test expectations.

Mitigation:

- version fixture manifests
- record snapshot versions in run artifacts
- keep per-service assumptions declarative and reviewable

### Coverage Explosion

Enumerating every public surface item can create a large matrix.

Mitigation:

- generate the matrix from catalogs and route registrations
- support profile-scoped execution
- allow selective reruns of failed cases

### Cleanup Failures

Containers or datasets may survive a failed run.

Mitigation:

- use run IDs consistently in names
- make teardown idempotent
- provide orphan cleanup commands

### False Confidence

A passing live run can hide missing CI-safe contract coverage.

Mitigation:

- keep live E2E additive, not substitutive
- continue to require SDK, dispatch, and surface adapter tests

## Recommended Implementation Sequence

1. define the fixture definition format
2. implement the `backup-node` host orchestration contract
3. implement manifest emission and teardown
4. stand up one profile end to end
5. implement catalog-driven live case generation
6. execute CLI, MCP, and API live passes for that profile
7. add structured reporting
8. expand profile coverage service by service

## Open Questions

These questions do not block the design but should be resolved during planning:

- whether the host orchestrator is implemented as shell scripts, Rust tooling, or a thin remote agent
- whether Docker Compose or direct Docker API control is the better host-side primitive
- where live artifacts should live in the repo workspace
- how fixture manifests should encode known entities and IDs for destructive and failure-path cases

## Decision

Proceed with a dedicated ephemeral live test environment on SSH host `backup-node`, using per-run Docker stacks backed by ZFS clones from golden fixture snapshots. Treat that environment as the canonical automated live target for end-to-end testing across CLI, MCP, and API surfaces.
