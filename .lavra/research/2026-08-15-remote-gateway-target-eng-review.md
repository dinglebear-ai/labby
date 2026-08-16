# Engineering Review: lab-hs69m - Make remote gateway target resolution authoritative

## Architecture

- Strengths: one shared resolver, typed explicit/opportunistic authority, and thin consumers match Labby's existing dependency direction.
- Critical concern: authority must survive detection. `gateway list` decode errors and remote Code Mode errors currently fall back locally after a daemon was selected.
- Important concerns: the plan named the wrong stdio file and omitted proxy OAuth and doctor preflight callers; child beads lacked dependency ordering.
- Changes required: carry authority in `LiveGateway`, audit every caller, make explicit post-selection failures terminal, and serialize `.1 -> .2 -> .3 -> .4`.

## Simplicity

- Prefer `enum TargetSet { Explicit { url, source }, Opportunistic(Vec<Url>) }` over a mode field that permits invalid combinations.
- Keep the production delta focused: shared resolver, safe endpoint construction, typed detection result, caller propagation, and regression tests.
- Do not add a pure stdio decision helper solely for tests; exercise the existing `cli/serve.rs` path through its current seams.
- Regenerate docs only if `docs-check` or metadata changes require it.

## Security

- Critical: Reqwest follows redirects by default. Set `redirect::Policy::none()` and prove redirect targets receive no request or bearer token.
- Treat the target URL and inherited token as one trusted operator authority domain. Reject userinfo, query, fragment, unsupported schemes, and non-loopback HTTP; redact rejected input.
- Use documented stable error kinds and recovery metadata. Do not introduce `upstream_unavailable` without changing the error contract.
- Emit one sanitized structured terminal probe event; never log raw environment values or credentials.

## Performance

- Bound the complete opportunistic discovery budget; per-request timeouts alone allow multi-second sequential startup delays.
- Bound MCP initialization and Code Mode calls after HTTP detection so a half-alive daemon cannot hang startup indefinitely.
- Reuse the action catalog fetched during identity detection for the proxy OAuth required-capability check.
- Do not add global clients, caches, HashSet deduplication, or mutation timeouts in this change.

## Failure Modes

| Codepath | Failure mode | Rescued? | Test? | User sees? | Logged? |
|---|---|---:|---:|---|---:|
| Explicit URL parse | Credentialed or malformed URL | Yes, validation error | Required | Visible | One terminal event |
| HTTP client | Redirect to another origin | Yes, redirect rejected | Required | Visible for explicit | One terminal event |
| Explicit probe | DNS, TLS, timeout, wrong service | Yes, typed error | Required | Visible | One terminal event |
| Explicit auth | Actions endpoint returns 401/403 | Yes, auth kind | Required | Visible | One terminal event |
| Opportunistic probes | Stale candidates consume startup budget | Yes, overall deadline then local fallback | Required | Bounded delay | Summary only |
| Gateway dispatch | Daemon fails after probe | Yes, propagate | Required | Visible | Dispatch event |
| Gateway list | Version-skew payload | Yes, propagate for explicit | Required | Visible | Dispatch event |
| Code Mode | MCP connect/call stalls or fails | Yes, timeout/propagate for explicit | Required | Visible | Warning/error |
| Stdio bridge | Daemon stalls after HTTP probe | Yes, initialization timeout | Required | Nonzero startup | Error |
| Proxy OAuth | Required capability absent | Yes, cached capability check | Required | Visible | Error |
| Doctor preflight | Explicit target invalid/unreachable | Yes, failed finding | Required | Report finding | Summary event |

No row remains both unrescued, untested, and silent after the required changes.

## Not in Scope

- CLI `--server-url` flag: environment/plugin configuration covers the reported defect.
- Response-source metadata on all action envelopes: routing failures and logs provide sufficient proof.
- Automatic config migration or persistence: no migration is required to repair resolution.
- Target-scoped token storage, certificate pinning, and private-network allowlists: worthwhile defense-in-depth, but remote LAN/tailnet targets are legitimate and need separate design.
- Concurrent/staggered public probing: an overall deadline bounds current cost without adding ordering complexity.
- Global HTTP client/cache, HashSet deduplication, metrics, or mutation timeouts: no demonstrated need in this repair.

## Summary

- Critical issues: 2 - redirect safety and post-detection fallback.
- Important suggestions: 8 - caller audit, endpoint joining, error taxonomy, observability, bounded discovery/MCP startup, capability reuse, dependency order.
- Minor improvements: 2 - enum simplification and conditional docs generation.

## Recommended Changes

1. Make explicit authority end-to-end: resolution, probe, dispatch, decode, MCP bridge, and Code Mode.
2. Disable redirects, use safe URL joining, and pin redacted typed failure behavior with tests.
3. Audit all callers and bound discovery/MCP initialization while reusing discovered capabilities.

## Completion Summary

```text
Architecture issues: 6  |  Simplicity: 4  |  Security: 6  |  Performance: 5
Critical gaps: 2  |  Deferred items: 6
```
