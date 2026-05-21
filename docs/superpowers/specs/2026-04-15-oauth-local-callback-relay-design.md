# OAuth Local Callback Relay Design

**Date:** 2026-04-15

## Goal

Add a local OAuth callback forwarder to `lab` so a machine running the browser can accept localhost OAuth redirects and forward them to another machine that is running the real OAuth callback listener.

## Context

Some MCP clients, notably Claude Code, allow operators to pin the OAuth callback port but not the full callback URL. That means the browser is redirected to a loopback URL such as `http://127.0.0.1:38935/...` even when the actual MCP client is running on another machine.

Today the working patterns are split across separate tools:

- the public callback relay on `squirts` forwards machine-specific callback URLs to registered targets
- helper scripts manage SSH localhost tunnels for browser-local redirects
- Codex can avoid this problem entirely when its callback URL is configurable

`lab` already exists on the operator machines involved in this workflow. The missing capability is a small browser-side helper that can listen on localhost and forward the callback request to the real target machine without reimplementing OAuth.

## Decision

Add a generic local callback forwarder to `lab` and expose a machine-aware CLI on top of it.

The forwarder will:

- bind to `127.0.0.1:<port>`
- accept arbitrary callback paths and query strings
- forward the request to a resolved target base URL
- preserve method, path suffix, query string, and body
- strip hop-by-hop headers
- return the upstream response unchanged where safe

This is preferred over a Claude-specific implementation because the transport behavior is generic and can serve any localhost-based OAuth callback flow.

## Non-Goals

This design does not include:

- replacing the existing public callback relay service
- automatic editing of Claude Code configuration files
- automatic discovery of a client's live callback port
- a browser UI
- OAuth token minting, code exchange, or PKCE logic inside `lab`

The forwarder is transport-only. It moves the final callback to the correct machine while the real client remains responsible for state validation and token handling.

## Existing Proven Patterns

The existing Python callback relay already validates the transport behavior we want to preserve:

- file-backed machine registry
- request forwarding with exact suffix-path and query preservation
- hop-by-hop header stripping
- minimal target-identifying header injection
- consistent JSON error responses

The existing SSH tunnel helper scripts validate the operator workflow for browser-local localhost callbacks. Those scripts show that the browser-side problem is distinct from the public relay problem and should remain a focused transport primitive.

The Rust design should port those proven forwarding semantics, not invent a new protocol.

## Architecture

### System Boundary

The new feature lives entirely in the `lab` binary as an operator-side helper. It is not part of the HTTP API or the MCP server surface. It is a CLI-run utility mode that opens a temporary local HTTP listener and forwards incoming callback requests to another HTTP target.

That keeps the feature operationally simple:

- no new long-lived public service is required
- no new OAuth behavior is embedded into `lab serve`
- no change is required in the remote MCP client beyond keeping its callback listener alive

### Core Units

#### `oauth::local_relay`

This module owns the loopback listener and forwarding behavior.

Responsibilities:

- bind a loopback socket on the requested port
- accept `GET` and `POST` callback requests on arbitrary paths
- construct the forwarded target URL from the resolved base plus suffix path and query string
- proxy the request body and filtered headers
- return the upstream response with hop-by-hop headers removed

It should be independent of Claude Code, Codex, or any specific OAuth provider.

#### `oauth::target`

This module resolves where forwarded callbacks go.

Supported resolution modes in the first cut:

- explicit `--forward-base <url>`
- machine lookup by ID from `lab` config

The resolved target is a base URL such as:

- `http://100.88.16.79:38935/callback/dookie`
- `https://dookie.tailnet.ts.net/callback/dookie`
- `https://callback.tootie.tv/callback/dookie`

When the incoming request path contains additional suffix segments, they are appended to the target base in the same way as the existing Python relay.

#### `oauth::config`

This module extends `LabConfig` with durable machine records for OAuth callback forwarding.

Each machine record should be able to hold:

- machine ID
- target base URL
- optional description
- optional default callback port

This avoids maintaining a second ad hoc registry file outside normal `lab` config.

#### `cli::oauth`

This module provides operator-facing commands.

The initial command surface should include at least:

- `lab oauth relay-local --port <port> --forward-base <url>`
- `lab oauth relay-local --machine <id> --port <port>`

Follow-on machine management commands can be added later, but the first implementation can read machine records from config without adding a full CRUD CLI immediately.

## Config Design

`LabConfig` should gain a new top-level section for named OAuth callback targets.

Proposed shape:

```toml
[oauth.machines.dookie]
target_url = "https://dookie.tailnet.ts.net/callback/dookie"
description = "Dookie Claude callback target"
default_port = 38935
```

Requirements:

- machine IDs are stable keys
- `target_url` is required
- `description` and `default_port` are optional
- env var overrides are not required for the first cut

Config resolution errors must be explicit and list available machine IDs when a requested machine is missing.

## Request Forwarding Rules

The transport behavior should intentionally mirror the existing Python relay.

### Path Handling

If the configured target base is:

`http://100.88.16.79:38935/callback/dookie`

and the browser hits:

`http://127.0.0.1:38935/callback/dookie/foo/bar?code=1&state=2`

the forwarded request should target:

`http://100.88.16.79:38935/callback/dookie/foo/bar?code=1&state=2`

If the configured target base already contains a query string, the incoming query string should be appended after it, matching the Python implementation.

### Header Handling

The forwarder should strip hop-by-hop headers from both inbound forwarded requests and outbound proxied responses. The stable denylist should match the existing Python implementation:

- `connection`
- `content-length`
- `host`
- `keep-alive`
- `proxy-authenticate`
- `proxy-authorization`
- `te`
- `trailer`
- `transfer-encoding`
- `upgrade`

The forwarder may add one diagnostic header such as:

- `x-lab-oauth-relay-machine-id`

### Methods and Bodies

The first cut should support at least `GET` and `POST`, because those cover normal OAuth callback flows and parity with the Python relay.

The forwarded body should be preserved byte-for-byte.

## Error Handling

Failures must be operator-readable and precise.

### Startup Errors

- missing `--machine` target in config
- malformed target URL
- invalid port
- bind failure on `127.0.0.1:<port>`

These should fail before the server starts and explain exactly what was wrong.

### Runtime Errors

- target machine unreachable: return `502 Bad Gateway`
- upstream timeout: return `504 Gateway Timeout`
- upstream returned non-2xx: preserve upstream status and safe response body
- unsupported method: return `405 Method Not Allowed`

The forwarder must not log or echo raw `code`, `state`, access tokens, or refresh tokens in user-visible output.

### Error Body Shape

Because this is a CLI-local utility, the returned HTTP error body can be a small JSON envelope rather than the full `lab` API error format. It should include a concise `detail` string and, where useful, the resolved machine ID or target host.

## Logging and Observability

The forwarder should use normal `tracing` and follow the repo observability rules.

Required startup log:

- local bind address
- resolved machine ID, if any
- resolved target host/path

Required per-request log:

- surface = `oauth_relay`
- method
- local path
- machine ID or explicit target
- target host
- response status
- elapsed time

Sensitive callback query values must be redacted. Logging query keys only is acceptable; logging the raw query string is not.

## CLI UX

The first-cut UX should prioritize reliability and clarity over feature breadth.

Recommended commands:

```bash
lab oauth relay-local --forward-base http://100.88.16.79:38935/callback/dookie --port 38935
lab oauth relay-local --machine dookie --port 38935
```

Behavior:

- bind only to `127.0.0.1` by default
- print the resolved forwarding target at startup
- run until interrupted
- exit non-zero on startup errors

The CLI should not attempt to launch the OAuth client or browser. It only hosts the temporary local forwarder.

## Testing Strategy

### Unit Tests

- target URL construction with suffix paths
- target URL construction with merged query strings
- hop-by-hop header stripping
- config machine lookup and missing-machine errors

### Integration Tests

- local relay forwards path/query/body to a mock upstream exactly once
- relay returns upstream status/body/content-type correctly
- unreachable target becomes `502`
- timeout becomes `504`
- bind collision returns a clear startup error

### Operator Smoke Test

- start a mock upstream callback receiver
- start `lab oauth relay-local`
- send a request to `127.0.0.1:<port>/callback/test?code=x&state=y`
- verify the upstream receives the exact suffix path and query

## Risks

### Scope Drift

It is tempting to absorb the public relay and machine onboarding in the same change. That would slow the first useful cut. The mitigation is to keep the initial implementation focused on the browser-local helper only.

### Secret Leakage in Logs

OAuth callback requests contain sensitive query values. The mitigation is to log request path and query keys only, never raw callback parameters.

### Ambiguous Target Semantics

If target configuration mixes base callback URLs and root URLs without a clear contract, forwarding behavior will become confusing. The mitigation is to define `target_url` as the callback base URL, not just a host.

## Migration and Follow-On Work

After the first cut is in place, follow-on work can include:

- machine CRUD commands under `lab oauth machine ...`
- optional import/export compatibility with the existing public relay registry
- a dedicated public relay mode inside `lab`
- onboarding helpers for Claude Code and Codex configuration

Those should remain out of scope for the initial implementation.
