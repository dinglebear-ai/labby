# Agent MCP configuration reference

Always inspect the installed client's `--help` plus current official documentation before mutation. Client MCP configuration formats and OAuth behavior change independently of Labby.

## Important distinction

There are two opposite directions:

1. **Labby imports MCP servers from an agent config**
   - `labby gateway discover` is read-only discovery.
   - `labby gateway import` imports selected discovered upstreams into Labby's gateway.

2. **An agent consumes Labby as its MCP server**
   - this is configured with that agent's native MCP client surface.
   - gateway discovery/import does not perform this registration.

Do not conflate the two.

## Claude Code

Current Claude Code supports remote HTTP MCP servers and OAuth login.

The checked-in Labby plugin has a compatibility MCP definition in `.mcp.json`. That static definition includes:

```json
"Authorization": "Bearer ${user_config.api_token}"
```

Treat that definition as a **bearer transport convenience**.

Current Claude Code behavior is security-relevant: when an explicit
`Authorization` header is configured and rejected, Claude does not silently
fall back to the OAuth flow. Therefore a static plugin definition that always
emits an Authorization header cannot transparently represent an OAuth-only
Labby client.

### Bearer-only Labby

Use one supported bearer configuration path:

- the Labby plugin MCP entry with its protected `api_token`, or
- a native `claude mcp add` registration whose token is sourced from protected
  environment/credential storage.

Prefer the CLI over hand-editing user JSON. Never place a Labby bearer token in
a tracked project configuration.

### OAuth-only Labby

Do **not** rely on the plugin's static bearer-header MCP entry.

Instead:

1. inspect current official Claude Code MCP docs and `claude mcp --help`;
2. add the Labby Streamable HTTP endpoint through a user/local client
   registration **without** a static Authorization header;
3. choose a scope that takes precedence over the plugin-provided server when
   the same endpoint/name would otherwise collide;
4. perform the current supported OAuth login, for example the client-native
   `claude mcp login` flow when available;
5. verify with the client's MCP listing/status surface;
6. perform a read-only Labby help call.

Claude's normal scope precedence means an explicit local/project/user server can
override or deduplicate a plugin-provided server. Confirm the exact current
behavior from official docs before applying it.

### OAuth + bearer Labby

Ask which credential path the user wants this Claude client to use:

- **OAuth identity**: configure it exactly like OAuth-only, with no static
  Authorization header.
- **Static break-glass bearer**: configure it like bearer-only.

Do not silently choose the break-glass credential for normal interactive use
merely because one exists.

## Codex

Codex is an MCP client. Codex App Server is a separate agent/application
protocol and is not an MCP server.

For Labby as a Codex MCP server:

1. inspect current official OpenAI Codex MCP docs and `codex mcp --help`;
2. add the Labby Streamable HTTP endpoint with the current supported MCP client
   command/config surface;
3. for bearer auth, prefer the current bearer-token environment-variable
   reference mechanism rather than embedding the secret;
4. for OAuth auth, use Codex's current MCP OAuth login/authentication flow;
5. verify with `codex mcp list` or the current equivalent;
6. invoke a read-only Labby help action.

For Phoenix LLM assistance, use Codex App Server separately. Do not register
`codex app-server` as a Labby MCP upstream.

## Other clients

Labby's gateway discovery recognizes client configurations including Claude
Code, Codex, Cursor, Claude Desktop, Windsurf, OpenCode, VS Code, and Gemini.
Recognition for gateway import does not guarantee that every client has the
same remote-MCP auth model.

To identify installed clients without mutating them, combine `labby gateway discover --json` evidence with read-only executable/config probes. Discovery currently scans Cursor, Claude Code, Claude Desktop, Codex, Windsurf, OpenCode, VS Code, and Gemini config families. A client can still be installed even when discovery returns no rows because its MCP config may be empty, so do not treat an empty discovery result as proof that the client is absent.

For each installed client:

1. identify the exact client/version;
2. find current official MCP documentation;
3. inspect its native MCP CLI/config surface;
4. show the operator the intended mutation;
5. keep tokens out of tracked config where the client supports env/credential
   references;
6. configure Labby;
7. restart/reload if required;
8. prove one read-only Labby action through the client.

If direct MCPorter succeeds while the agent client fails, stay in the client
configuration/authentication layer. Do not reinstall Labby.
