# Native Skills migration

Production SDK: [`dc4c6da`](https://github.com/dinglebear-ai/rust-sdk/commit/dc4c6dac4a25ef91b4dab795df799dc008fb3661), RMCP `=3.9.0`.

The candidate is published in [SDK fork PR #9](https://github.com/dinglebear-ai/rust-sdk/pull/9). Fork `main` remains `01537b5`; Labby publication is separate.

Labby uses native list/get/directory handler types and native client requests. The old raw-response hooks are removed. Existing caller/route policy, partial-catalog validation, metadata, pagination, cancellation and response budgets remain in their current owners.

Unknown inbound cache hints retain usable Skills. Outbound hints the SDK cannot represent become `private`; this can reduce sharing but cannot broaden it. Explicit Unix JSON limits preserve the supported 24 MiB Skills response allowance; error bodies remain capped at 64 KiB or the tighter configured limit.

The SDK also retains the optional OAuth code consumed by the existing production conformance runner. Its pinned `@modelcontextprotocol/conformance@0.2.0-alpha.12` client extension suite passed all **58 assertions across six scenarios**, without a failure baseline. Full SDK all-feature/no-local suites, both strict Clippy configurations, schema snapshots and nightly formatting passed.

The Events field is capability plumbing. MCP Events webhook delivery, Claude Channels delivery, scheduling and watchers remain in the separate delivery plan.

## Upstream candidates

Each candidate is based on upstream `f2b0ddf113d1ed426b77a81511eac6e4bffa0ac4`. The branches are published and remote revisions verified. GitHub rejected upstream PR creation with the current CLI credentials (GraphQL permission error; REST 404); the app connection requires reauthentication. No upstream PR has been created.

| Change | Candidate | Validation |
| --- | --- | --- |
| OAuth JWT future is Send | [Compare `de75615`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-auth-send-20261108?expand=1) | 1 Send regression |
| Strict Origin validation | [Compare `345c333`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-origin-validation-20261108?expand=1) | 29 no-local Origin integration tests |
| Explicit Unix JSON budgets | [Compare `e5cfd70`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-unix-response-budget-20261108?expand=1) | 7 no-local Unix transport tests |
| Opt-in Events capability | [Compare `aa1720b`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-events-capability-20261108?expand=1) | 2 capability tests and schema snapshots |
| Bounded SSE retries | [Compare `1d9191b`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-sse-retry-policy-20261108?expand=1) | 23 SSE tests |
| Workload JWT / enterprise client | [Compare `9128862`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-workload-jwt-grant-20261108?expand=1) | 4 workload JWT tests; combined conformance below |
| Repeated authentication challenges | [Compare `dad7434`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-repeated-auth-challenges-20261108?expand=1) | 6 no-local Unix/HTTP tests |
| Bound DPoP clients and nonce recovery | [Compare `1b26339`](https://github.com/modelcontextprotocol/rust-sdk/compare/main...dinglebear-ai:codex/upstream-oauth-dpop-20261108?expand=1) | 12 DPoP tests in each feature mode; combined conformance below |

## Labby qualification

Final local qualification against the published SDK revision passed:

| Check | Result |
| --- | --- |
| Workspace all-feature compilation | Pass |
| Strict workspace Clippy, formatting, drift, toolchain and module reachability | Pass |
| Feature-slice script | All 21 compile configurations pass |
| Gateway all-feature suite | 1,754 passed; 5 skipped |
| Auth all-feature suite | 723 passed |
| MCP library suite | 622 passed |
| Real-process Skills, multi-hop proxy and MCP primitive scenarios | 18 passed; shared harness self-tests excluded |
| Documentation gates | Pass |
| Conformance pin/workflow guard tests | 32 passed using CI-pinned PyYAML 6.0.2 in a task-local environment |
| SDK CI minimal gate | 188 passed |
| Production SDK OAuth extension conformance | 58 assertions passed across 6 scenarios |

The real-process cases cover initialized and discovered HTTP/stdio clients,
native list/get/directory reads, manifests and supporting-file integrity,
pagination, catalog replacement, OAuth code exchange/refresh, and trust-policy
rejection. They use owned Labby processes and controlled provider fixtures.
End-client Events/Channels wake behavior and deployment belong to later delivery
qualification; frontend assets were not built for this backend migration.

The exact working branch is `codex/mcp-events-channels-20261108`, based on
`40129e9b3bf79a21708f3354a3734133abc88b13`. The migration is committed locally;
Labby publication remains separate. SDK fork PR #9 is open at the revision above;
no hosted checks were present when inspected, and no PR was merged.

An extra exploratory `client,server` SDK test configuration without macros hits
unchanged upstream macro-dependent test helpers. It is outside Labby's feature
configuration. The CI-owned no-default-feature minimal command and the full
all-feature/no-local configurations above passed.

The independent review caught and fixed a directory-capability mismatch: SDK dispatch now reads the same namespace policy as discovery, and per-request async policy remains authoritative. A Service dispatch regression covers successful native-only gateway reads and revocation after a proxied namespace is enabled.

Cold HTTP regressions exposed large nested native SDK futures. Boxing the resource-read future inside the existing attribution boundary reduced stack use; all 622 MCP tests then passed with the normal stack limit. Temporary diagnostic source edits were removed.

## Decisions

- Preserve all actual consumers of the pinned fork, including optional OAuth used by conformance. Removing it would regress the existing strict gate.
- Preserve the 24 MiB Skills transport allowance with an explicit bounded SDK setting; retain the upstream 16 MiB default and the 64 KiB error ceiling.
- Preserve independent Skills DTOs and map unsupported outbound cache hints to private. Retaining a future hint verbatim would require another SDK model change; private may cost cache reuse.
- Keep the stock SDK conformance baseline unchanged; the production fork pin belongs in the manifests and conformance shell script.
- Publish SDK candidates for review; keep Labby changes local and leave all PR merges to a human.
