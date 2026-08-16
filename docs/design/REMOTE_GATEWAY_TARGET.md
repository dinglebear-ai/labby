# Remote Gateway Target Resolution

## Problem

Labby can run as a local daemon or as a thin client of an existing daemon. The
Labby plugin already supplies `CLAUDE_PLUGIN_OPTION_SERVER_URL` to its MCP
transport, and setup uses that value for connectivity checks. Gateway CLI and
stdio daemon discovery do not currently consume it. If their opportunistic
probe fails, they silently create a local gateway view and read another
`config.toml`, which can contradict the connected daemon and Code Mode catalog.

## Required behavior

- Treat a non-empty `CLAUDE_PLUGIN_OPTION_SERVER_URL` as an explicit remote
  Labby daemon target for plugin-launched processes.
- Add `LABBY_SERVER_URL` as the product-owned equivalent for ordinary CLI and
  stdio clients. The plugin option has precedence when both are present because
  it is the invocation-scoped user choice.
- Normalize explicit targets to a daemon base URL. Accept an origin/base URL or
  a URL ending in `/mcp`; strip only the terminal `/mcp` path. Reject userinfo,
  query strings, fragments, unsupported schemes, and non-loopback plaintext
  HTTP.
- Preserve `LABBY_MCP_GATEWAY_URL` and `LABBY_PUBLIC_URL` as opportunistic
  compatibility discovery candidates. They retain their existing daemon-side
  meanings and do not become aliases for the new explicit client target.
- When an explicit remote target is configured, gateway CLI and stdio startup
  must either use that daemon or return a structured, actionable error. They
  must not fall back to a local `config.toml`.
- When no explicit target is configured, preserve the existing local-bind and
  public-URL probes followed by standalone local fallback.
- Never send `LABBY_MCP_HTTP_TOKEN` to a different origin after a redirect.
  The shared remote client must use `reqwest::redirect::Policy::none()` for
  health, identity, dispatch, and MCP initialization requests.
- Build health, action, dispatch, and MCP endpoints with `Url::join`; raw URL
  string concatenation is prohibited.
- Explicit authority survives successful detection: later authentication,
  response decoding, dispatch, MCP initialization, or Code Mode failures must
  be returned to the caller and must never trigger local execution.
- Bound the complete opportunistic discovery attempt and MCP initialization so
  stale or half-alive endpoints cannot stall one-shot commands or stdio startup
  indefinitely.
- Classify probe failures by stage using existing stable error kinds. Emit one
  sanitized terminal probe event with source name, origin, elapsed time, and
  kind; never log the raw environment value.
- Human and JSON errors must identify the configured endpoint without exposing
  credentials or tokens and must explain that local fallback was intentionally
  suppressed.
- The same resolver must be used by gateway CLI commands and stdio thin-client
  startup so those surfaces cannot drift again.

## Non-goals

- Do not merge daemon bind configuration (`mcp.host` / `mcp.port`) with client
  target configuration.
- Do not change the persisted upstream schema or migrate either existing
  `config.toml` automatically.
- Do not make `LABBY_PUBLIC_URL` fail closed; it remains both daemon metadata
  and an opportunistic compatibility candidate.
- Do not add a CLI flag in this change. Environment/plugin configuration covers
  the reported inconsistency without expanding every command's clap surface.
- Do not add response-source metadata to every gateway action envelope; clear
  routing failures and regression coverage are sufficient for this repair.
- Do not add concurrent public probing, global client caching, metrics,
  certificate pinning, target-scoped token storage, or private-network
  allowlists in this repair.

## Acceptance criteria

1. A plugin-launched `labby gateway get tidewave` reaches the configured remote
   daemon even if the invoking user's XDG config omits Tidewave.
2. An unreachable explicit target produces a structured error and does not
   instantiate or read the local gateway manager.
3. With no explicit target, bootstrap/offline local fallback behaves exactly as
   it does today.
4. `LABBY_SERVER_URL` and `CLAUDE_PLUGIN_OPTION_SERVER_URL` share validation,
   normalization, redaction, probing, and dispatch behavior.
5. Targeted tests, all-features checks, lint, and generated-doc checks pass.
6. Redirects are not followed, post-detection explicit failures never execute
   locally, overall discovery is bounded, and stalled MCP initialization fails
   within its configured startup timeout.
