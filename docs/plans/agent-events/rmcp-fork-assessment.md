# RMCP fork exit assessment

> Historical assessment snapshot. The [native Skills migration record](native-skills-migration.md) tracks the implemented adapters, current SDK pin, validation and upstream candidates.

Investigated on 2026-11-08. This is a dependency assessment, not an implemented
migration or a qualification of Labby against stock RMCP.

## Decision

Keep the fork for the current Labby code. Its original metadata workaround is
now replaceable, but switching directly to upstream would lose required APIs
and retained behavior. A migration can remove most of the raw-response patch;
an OAuth compilation defect still needs fixing upstream or retaining locally.

Sources examined:

- Labby: `87d0eff0dd1de9aa8543174fa15d031072326eac`, with production RMCP still
  pinned to fork `=3.7.0` / `c8f2ac4cb10ac7882bfb0c7ca048977c3dde91e2`.
- Fork main: [`01537b5`](https://github.com/dinglebear-ai/rust-sdk/commit/01537b5abaaaa055e439d5382a2f6c989ce2912e).
- Current upstream main examined: [`9c21b34`](https://github.com/modelcontextprotocol/rust-sdk/commit/9c21b348d717690e65b9be741b401e0961e5d275),
  one Skills-catalog addition after the v3.9.0 release.

## What can be removed

Upstream's [`_meta` correction](https://github.com/modelcontextprotocol/rust-sdk/pull/1405)
preserves custom result payloads. Its [native Skills support](https://github.com/modelcontextprotocol/rust-sdk/pull/1392)
provides `Peer::list_skills`, `Peer::get_skill`, and directory reads. An actual
client/server exchange preserved frontmatter extras, entry/list metadata,
dynamic manifests, pagination, private cache scope, and the unknown-skill error.

Labby's `send_request_as` calls therefore have a viable replacement for these
current uses. That replacement also lets the HTTP, WebSocket, stdio, and ordered
relay adapters stop depending on fork-only raw-response plumbing. Six gateway
production modules reference that plumbing: `http_client.rs`,
`pool/skills_list.rs`, `pool/stdio_transport.rs`, `pool/connect.rs`,
`transport/websocket.rs`, and `transport/unix_socket.rs`.

The migration still needs both sides of Skills dispatch. RMCP 3.9 handles
`skills/list`, `skills/get`, and `resources/directory/read` before custom methods;
Labby's server currently implements them through `on_custom_request`.
Retain the existing bounded listing/cancellation wrappers instead of using
unbounded `list_all_*` helpers.

One compatibility choice remains: Labby's DTO explicitly preserves unknown
`cacheScope` strings; upstream's typed DTO rejects them. This was confirmed with
`some-future-scope`. A direct DTO substitution would change that behavior.

## Remaining differences

| Area | Evidence | Consequence and smallest candidate path |
| --- | --- | --- |
| OAuth client credentials | `assert_send(manager.exchange_client_credentials(config))` fails with `E0277` against stock upstream and passes against the fork, including on Labby's Rust 1.97.1 pin. The diagnostic identifies `form_urlencoded::Serializer` in the JWT branch. | A real remaining compilation problem for the JWT-enabled OAuth stack. Upstream the existing scoped-serializer fix; retain it until available. Disabling the feature would remove supported authentication behavior. |
| Origin validation | Real RMCP service requests with two Origin headers, a trailing path, or a non-default port returned `200` from stock and `403` from the fork under the same allowlist. | Preserve the stricter policy. Candidate alternatives are upstream fixes or a Labby ingress check for duplicate/malformed values plus explicit default ports in generated allowlists. These requests do not demonstrate an authentication bypass. |
| Unix-socket response limits | Stock has fixed JSON/error limits of 16 MiB/64 KiB and no `with_max_response_bytes`. A 1 KiB response passes stock, while the fork rejects it with a configured 128-byte limit. | Post-deserialization size checks cannot replace a transport byte bound. Upstream a configurable limit or reuse Labby's bounded HTTP client with reqwest's native Unix connector. |

The reqwest alternative is more than an API guess: an isolated HTTP exchange
over both filesystem and Linux abstract sockets preserved the request path,
query, and Host header without DNS. Labby's existing `BodyCappedHttpClient`
accepts a configured reqwest client. Reusing that owner is a plausible route
without implementing another HTTP framing/SSE client. This does not yet prove
Labby's OAuth, cancellation, headers, redirects, or SSE behavior over that route.

## Upgrade caveats

Labby's default Skills transport budget is **24 MiB**; its ordinary response
budget is 10 MiB, with an explicit operator setting overriding both. Stock
RMCP's Unix JSON ceiling is **16 MiB**. The merged fork also takes the minimum
of its configured cap and that upstream ceiling. Thus the ceiling is an upgrade
compatibility issue for either v3.9 choice, not something the merged fork alone
solves. The currently pinned Labby dependency remains unchanged.

Before changing that pin, qualify a valid Skills response between 16 and 24 MiB
and a response above the configured limit. Preserve the current documented
budget or explicitly resolve the changed contract; do not treat SDK-only test
success as proof of this Labby behavior.

The planned MCP Events work adds another separate SDK prerequisite: current
upstream and fork main have no top-level `ServerCapabilities.events` field.
That capability support needs an SDK change regardless of whether the existing
raw-response patch is removed.

## Proof and scope

G1: determine whether stock RMCP can replace the fork without losing Labby's
current supported behavior. Source inspection and the focused counterexamples
close the investigation; G1 is not satisfied by a dependency-only switch.

The isolated assessment fixtures in `target/rmcp-fork-assessment/` ran six checks
against stock and six against the fork. The OAuth `Send` assertion is a separate
expected compilation failure on stock and a successful compilation on the fork.
Logs, manifests, SDK source identity, and a receipt remain in that ignored
task-owned directory. No real credentials or external service requests were used.

No matching dinglebear-ai/Labby/rmcp Decision or Trust Boundary records were
returned by the skill knowledge index as of this date; repository source,
contracts, and applicable instructions supplied the local evidence. No product
source, dependency pins, instruction files, deployment, or remote publication
were changed by this investigation. Full Labby compilation, feature suites,
and native client acceptance remain migration qualification work.
