# Specification: universal Skills over MCP compatibility

Status: active
Created: 2026-08-18
Last updated: 2026-08-18

## 1. Problem statement

Labby already speaks the draft SEP-2640 Skills extension natively, but most MCP clients do not yet consume that extension as a first-class skill source. A client may support MCP tools, resources, prompts, or local Agent Skills while still ignoring io.modelcontextprotocol/skills.

Without a compatibility layer, the same skill must be manually copied, wrapped in ad-hoc tools, or installed separately for each client. That creates drift, duplicated trust decisions, duplicated update logic, and transport-specific behavior.

Labby needs one canonical skill model that can be projected into the capabilities a client actually has.

## 2. Existing foundation: do not rebuild

The following already exists on main and is explicitly out of scope for reimplementation:

- transport-neutral SEP-2640 DTOs and validation in labby-runtime
- native extension capability advertisement
- native skills/list and skills/get handling
- first-party bundled Labby skills
- operator-loaded local skills
- upstream Skills capability discovery
- upstream pagination and caching
- per-subject cache isolation
- manifest completeness validation
- digest verification and frontmatter cross-verification
- URI parsing and lossless origin relabelling
- route-scoped upstream aggregation
- proxied skill resource reads
- skill-specific structured error kinds
- conformance tests for the current pinned SEP revision

The existing docs/contracts/skills-extension.md remains the normative wire contract for those behaviors.

## 3. Goal

A skill registered once with Labby should be usable through the strongest safe projection supported by a downstream client, without changing the skill content or maintaining client-specific copies.

The compatibility system must preserve progressive disclosure: clients discover compact metadata first and load full skill content only when needed.

## 4. Compatibility profiles

### 4.1 Native extension profile

For clients that understand io.modelcontextprotocol/skills:

- advertise the extension normally
- serve skills/list and skills/get
- serve manifest-bound files through resources/read
- preserve existing SEP behavior exactly

This remains the preferred path.

### 4.2 Tool projection profile

For any MCP client capable of tool calls, expose exactly one fixed service named skills using Labby's action plus params convention.

Required actions for the first usable slice:

- skills.list: enumerate compact skill metadata
- skills.search: rank/filter compact metadata without loading skill bodies
- skills.get: resolve one skill entry by published URI
- skills.read: read one manifest-bound file and return verified content

The fixed tool is a compatibility projection, not a competing skill registry.

### 4.3 Code Mode profile

When Code Mode hides ordinary direct MCP tools, the skills service must remain present in the internal Code Mode catalog. The model reaches it through Code Mode search/describe/call just like every other internal service.

No Code Mode-only skill data model is allowed.

### 4.4 Resource-aware profile

Clients that can manually enumerate/read MCP resources may consume skill files through the standard resource surface. Resource support alone does not imply automatic Agent Skill registration, so Labby must describe this as manual compatibility rather than native skill support.

### 4.5 Filesystem projection profile

A later explicit adapter may materialize verified skills into a client's local skill directory for clients whose native skill loader is filesystem-only.

This adapter must be opt-in, atomic, provenance-preserving, and generated from the canonical registry. It must never be the source of truth.

Initial targets may include Claude Code, Codex, and Gemini CLI, but the core API must use projection profiles rather than hard-coded client names.

## 5. Architecture

### 5.1 Single source of truth

The canonical registry is the merged view of:

- bundled Labby skills
- operator-local Labby skills
- route-allowed, enabled, skills-proxying upstreams

Every projection must use the same canonical list/get/read semantics and the same validation path.

### 5.2 Shared semantic layer

Shared skill operations belong in the transport-neutral product layer, not in CLI, MCP handlers, or HTTP routes.

Target dependency direction:

labby-runtime: wire/domain vocabulary and validation
labby-gateway: upstream discovery, fetch, cache, verification
labby shared skills/dispatch layer: canonical aggregate, list/search/get/read semantics
CLI/API/MCP/native extension: thin adapters over that layer

The current first-party registry living under crates/labby/src/mcp/skills.rs should be incrementally extracted rather than duplicated.

### 5.3 Fixed tool count

Skill cardinality must not affect MCP tool cardinality. Ten skills and ten thousand skills expose the same compatibility tool surface.

### 5.4 Progressive disclosure

list/search responses must not include SKILL.md bodies or supporting files by default.

get returns the skill entry and provenance metadata.

read returns exactly one verified file per request unless a future explicitly bounded batch action is added.

## 6. Functional requirements

### FR-1 Native behavior remains compatible

Existing SEP clients must observe no behavior regression when the fallback projection is enabled.

### FR-2 Tool-only clients can discover skills

A client that supports only MCP tools must be able to find relevant skills by list or search and retrieve their instructions using get/read.

### FR-3 Same identity everywhere

A skill has one published URI regardless of projection. Native skills/get and compatibility skills.get must resolve the same entry.

### FR-4 Same content verification everywhere

Compatibility reads of proxied files must enforce the same manifest and digest rules as native resources/read.

### FR-5 Route and auth parity

A compatibility caller must never discover or read a skill from an upstream excluded by the caller's route scope or authorization context.

### FR-6 Search is metadata-only

Search operates over names, descriptions, metadata, origin/provenance, and optional operator tags. It must not eagerly fetch every SKILL.md.

### FR-7 Stable output

All compatibility actions return stable structured JSON suitable for direct MCP use, Code Mode, CLI JSON, and HTTP JSON.

### FR-8 Honest incompleteness

If upstreams are unreachable, validation excludes entries, or collection is budget-truncated, list/search results must preserve incompleteness metadata rather than looking authoritative.

### FR-9 No client-specific mutation in P0

P0 does not write into Claude, Codex, Gemini, or other client directories. Filesystem projection is a later explicit phase.

## 7. Non-functional requirements

### NFR-1 Bounded work

Search/list must use existing caches and bounded concurrency. A compatibility query must not trigger unbounded upstream fan-out or unbounded resource reads.

### NFR-2 No secret leakage

Subject identifiers, bearer tokens, OAuth material, and upstream secret headers must not appear in returned provenance or logs.

### NFR-3 Determinism

Given the same registry snapshot and route scope, list/search ordering and result identity must be deterministic.

### NFR-4 Observability

Every user-visible compatibility action produces one structured dispatch event with surface, service, action, outcome, elapsed time, and redacted subject/origin context as applicable.

### NFR-5 Feature safety

Because SEP-2640 remains draft, native wire support stays feature-gated as today. Compatibility projection must be built so it can continue operating on the canonical Labby registry if the upstream draft changes, while never claiming conformance to a revision Labby has not validated.

## 8. Search semantics

P0 search should be deterministic and dependency-light.

Inputs:

- query: required non-empty text
- limit: optional bounded integer
- origin: optional origin filter

Ranking order:

1. exact skill name match
2. skill name prefix match
3. skill name token/substring match
4. description token match
5. metadata string match

Ties are ordered by origin label then published URI.

No embedding or remote ranking dependency is required for P0. A later Axon-backed semantic index may be added behind the same contract.

## 9. Error semantics

Compatibility actions use Labby's structured ToolError and agent recovery contract.

Important classes:

- invalid/missing params: revise_and_retry
- unknown skill URI: rediscover
- digest or manifest verification failure: rediscover, not permanent absence
- route-excluded origin: present as absent to avoid leaking hidden topology
- unavailable upstream: retry_later where appropriate, while aggregate list/search may succeed partially

The native SEP -32602 rule remains native-wire behavior and is not copied blindly into CLI/API adapters.

## 10. Acceptance criteria for the first implementation slice

- A shared action catalog exists for one skills service.
- skills.list, skills.search, skills.get, and skills.read are defined in shared dispatch.
- MCP exposes one skills tool through the ordinary service registry when enabled.
- Code Mode can discover that internal skills service.
- Native skills/list and compatibility skills.list resolve the same first-party skill identities.
- Native resource reads and compatibility skills.read return the same bytes for a first-party file.
- Route/auth gating tests prevent cross-route skill discovery.
- Existing native SEP conformance tests still pass.
- No per-skill MCP tools are introduced.

## 11. Deferred scope

- client-local filesystem export/sync
- automatic downstream client fingerprinting
- semantic/vector search
- UI installation buttons
- loadout UX, except shared integration points with the existing feature/skills-ui-config work
- new Skills-specific change notification beyond what the current SEP defines
- automatic execution of allowed-tools

## 12. References

- docs/contracts/skills-extension.md
- docs/surfaces/MCP.md
- docs/dev/DISPATCH.md
- docs/dev/OBSERVABILITY.md
- docs/dev/ERRORS.md
- modelcontextprotocol/modelcontextprotocol PR 2640
- modelcontextprotocol/experimental-ext-skills
