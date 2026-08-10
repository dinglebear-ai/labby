# Tool Annotations — Design Package

Design and implementation package for [issue #212](https://github.com/dinglebear-ai/labby/issues/212):
set MCP `ToolAnnotations` on Labby's own tools, and guarantee that upstream tools'
annotations reach downstream clients unchanged.

Tracked as beads epic `lab-g1av5` (children `lab-g1av5.1` … `lab-g1av5.4`).

## Read in this order

| Document | Answers |
|---|---|
| [REVIEW_FINDINGS.md](REVIEW_FINDINGS.md) | **Read first.** Reconciled output of two review rounds (11 agents): every correction applied, scope decisions, and the one gating unknown. |
| [SPEC.md](SPEC.md) | What we are building, why, what is explicitly out of scope, and the acceptance bar. |
| [CONTRACT.md](CONTRACT.md) | The externally observable promise: what Labby advertises, what it passes through, and what changes for clients. |
| [MODELS.md](MODELS.md) | The domain model — how an action-level `destructive` flag becomes a tool-level hint, and the four types already in the pipeline. |
| [SCHEMAS.md](SCHEMAS.md) | Wire-level JSON Schema for the annotations block and the exact serialized output per tool. |
| [TYPES.md](TYPES.md) | The concrete Rust types and signatures being added, with the rmcp 3.1.0 API they build on. |
| [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) | Step-by-step plan with real code, real file:line anchors, and the test matrix. |
| [PROGRESS.md](PROGRESS.md) | Live status tracker. Working document — update as work lands. |

## The one-paragraph version

Labby exposes **one MCP tool per service**, but destructiveness is recorded
**per action** (`ActionSpec.destructive`). MCP's `ToolAnnotations` are per-tool,
so a tool-level hint can only be the *least-safe union* of its actions. We set
all four hints explicitly on every Labby-owned tool, derive `destructiveHint`
from the live action catalog, and keep `readOnly`/`idempotent`/`openWorld` in a
reviewed constant table. Upstream annotations already flow through the gateway
untouched — that half of the issue is verified by tests, not changed by code.

## Three findings that shaped this design

**1. Half the issue is already implemented.** The whole `rmcp::model::Tool` —
annotations included — is cached in `UpstreamTool.tool` and moved to the
downstream client verbatim. No rebuild, no field stripping, no name-mangling.
Child bead `lab-g1av5.2` therefore adds *tests*, not a fix.

**2. The issue's line citation is stale.** It points at `helpers.rs:368`; the
function that consumes annotations is `cached_upstream_tool`, now at
`crates/labby-gateway/src/upstream/pool/helpers.rs:423`.

**3. Annotations are not advisory here — they gate authorization at the next
hop.** In a labby → labby chain the downstream gateway derives its destructive
gate *from these very hints* (`cached_upstream_tool`, fail-closed). That value
gates not just MRTR elicitation but a hard `forbidden` in Code Mode
(`code_mode_host.rs:90-107`) and the palette (`palette.rs:235-247`). Annotating a
builtin non-destructive therefore makes it **callable by a `lab:read` caller who
is forbidden today**. That is why every read-only claim is backed by a per-action
mutation audit ([SPEC § 5](SPEC.md#5-mutation-audit)), why `server_logs` carries a
documented override, and why neither "advisory only" nor "relaxes elicitation"
may appear in the shipped docs. Accepting this is the one open question gating
implementation ([SPEC § 7, F9](SPEC.md#7-safety-semantics-must-be-stated-correctly)).

## Scope boundary

In scope: annotation policy, the two mirrored construction sites, passthrough
tests, docs.

Out of scope: extending `ActionSpec` with `read_only`/`idempotent`/`open_world`
fields (per-action hints), and changing the `cached_upstream_tool` derivation or
the MRTR elicitation gate. Code Mode catalog hints are a stretch bead
(`lab-g1av5.4`, P3) that can be dropped without affecting the epic.
