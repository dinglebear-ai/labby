# Lab Observability Gaps

**270 total gaps: 20 Critical, 57 High, 112 Medium, 61 Low**

## Critical Gaps

1. `src/node/enrollment/store.rs` — validate/approve/deny/record_pending — zero logs on security lifecycle
2. `src/mcp/elicitation.rs` — elicit_confirm — outcomes never logged
3. `src/node/ws_client.rs` — connect_and_run_session — WS connect/init/close/errors silent
4. `src/node/ws_client.rs` — send_and_await — pending map saturation silent
5. `src/node/runtime.rs` — spawn_ws_flush_loop — no startup log, silent exit
6. `src/dispatch/logs/ingest.rs` — try_submit/spawn_writer — log events dropped silently
7. `src/dispatch/node/send.rs` — send_rpc_to_node — RPC cap hits produce no log
8. `src/dispatch/deploy/lock.rs` — acquire — lock contention timeout silent
9. `src/dispatch/deploy/build.rs` — build_release — entire cargo build lifecycle silent
10. `src/dispatch/marketplace/mcp_dispatch.rs` — dispatch_mcp_install — zero metrics
11. `src/node/log_store.rs` — ingest/open — blocking send under burst; SQLite open untraced
12. `src/mcp/services/nodes.rs` — enrollments approve/deny — zero audit trail
13. `src/dispatch/logs/store.rs` — open — SQLite open/WAL/migration silent
14. `src/oauth/upstream/manager.rs` — build_auth_client — token refresh invisible

## High Gaps (57 total)

Spans: node heartbeat/sync, gateway install/remove/refresh, MCP peer
registration, tool dispatch error paths, fleet health aggregation, deploy
runner lifecycle, ACP session teardown, upstream pool drain, sysmetrics
collection, identity resolution.

## Medium Gaps (112 total)

Spans: node store CRUD, marketplace sync polling, ACP bead creation, log
subscriber fan-out, gateway catalog diff, OAuth scope validation, enrollment
expiry sweeper, node update apply, rmcp transport framing, fleet command
broadcast, deploy stage transitions, bead state machine, TUI refresh ticks.

## Low Gaps (61 total)

Spans: config reload, static file serving, debug endpoint access, CLI
argument validation, tool schema generation, enrollment token rotation,
gateway health-check scheduling, log compaction triggers.

## Remediation Groups

| Group | Files | Priority |
|-------|-------|----------|
| Security audit logging | enrollment/store + elicitation + deploy/authz + mcp/services/nodes | Critical |
| Node WebSocket observability | ws_client + runtime spawn | Critical |
| Log ingest reliability | logs/ingest + logs/store + log_store | Critical |
| RPC backpressure visibility | dispatch/node/send + ws pending map | Critical |
| Deploy pipeline observability | build + lock + runner + stages | Critical |
| OAuth + token refresh | oauth/upstream/manager | Critical |
| MCP server lifecycle | peers + server + cli/serve | High |
| Upstream pool + gateway | upstream pool + gateway manager | High |
| Marketplace observability | mcp_dispatch + sync + acp_dispatch | High |
| Node fleet management | store + update + sysmetrics + identity | High |

## References

- ACP infinite recursion fix: `axon_rust/crates/services/acp.rs` (LABBY_SPAWN_DEPTH)
- Lab stdio guard: `crates/lab/src/cli/serve.rs` (LABBY_SPAWN_DEPTH guard)
- Marketplace dispatch: `crates/lab/src/dispatch/marketplace/dispatch.rs`
