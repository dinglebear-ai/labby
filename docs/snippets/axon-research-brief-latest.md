# implementing mcp-ui in rust

## Answer
Here’s the practical picture for implementing “mcp-ui” in Rust, based on the sources:

## 1) Start from the MCP server model, not the UI
The official MCP docs frame a server as exposing three core capability types: **tools**, **resources**, and **prompts**. Most UI-related work sits on top of those capabilities, especially tools, because they are what the host or UI can list and call [1].

If you are building a Rust MCP server, the official Rust SDK is the best anchor point: the `modelcontextprotocol/rust-sdk` repository is the official Rust implementation of MCP [9].

## 2) Rust server basics still matter for UI-heavy setups
Even if your end goal is UI, the server transport and logging rules still apply.

- MCP servers commonly use **stdio** for local desktop-style integration, or HTTP-based transports for web/networked setups [2].
- For **stdio-based servers**, never write logs to stdout, because that corrupts JSON-RPC messages. Log to **stderr** or to a file instead [1].
- For HTTP-based servers, stdout logging is fine because it does not interfere with the protocol stream [1].

So if your Rust UI is embedded into the server process, keep protocol I/O and logging cleanly separated.

## 3) There are two Rust paths for “mcp-ui”
The sources point to two related but distinct approaches:

### A. Embedded server UI / explorer
The `mcp_embedded_ui` crate provides a lightweight embedded web UI for any MCP server. Its docs say it serves:

- a self-contained HTML explorer page
- JSON API endpoints for listing tools
- endpoints for inspecting and executing tools [3]

It is built around Axum and exposes helpers such as:

- `build_ui_routes`
- `create_app`
- `create_mount` [3]

That means a Rust server can mount the UI directly into its HTTP app. The crate also includes types for:

- `UiConfig`
- `ToolSummary`
- `ToolDetail`
- `CallResult`
- `ValidationFailure`
- `AuthHook`
- `ToolsProvider`
- `Tool` [3]

This is the most direct interpretation of “mcp-ui in Rust”: a server-side web UI that introspects and runs MCP tools.

### B. MCP Apps / interactive widgets
The PMCP guide describes a richer “MCP Apps Extension,” where the server serves interactive widgets alongside tools [5]. In that model:

1. The **Rust server** registers tools and provides UI metadata.
2. The **widget** is a separate HTML/JS app that runs in the host iframe and talks back to the host [5].

Important details from the guide:

- enable the `mcp-apps` feature in `Cargo.toml`
- use a scaffolded project layout with a Rust server and a widget directory
- build the widget into **self-contained HTML** before embedding it
- the widget bundle must be self-contained because host iframe CSP may block external script loading [5]

This is the better path if you want rich interactive UI elements, not just a tool explorer.

## 4) Rust-side UI metadata is important
For MCP Apps, the server needs to attach UI metadata to tools. The PMCP guide shows that tools can be associated with widgets using `ToolInfo::with_ui()` and that host-specific metadata may be needed [5].

A notable point: the guide says that enabling a host layer such as `HostType::ChatGpt` adds `openai/*` keys to `_meta`, which ChatGPT requires, while Claude Desktop does not require them but tolerates them [5].

So for cross-host UI support, the Rust server should be prepared to emit host-specific metadata when needed.

## 5) Tool discovery and execution are the UI backbone
The embedded UI approach is centered on standard MCP tool flows:

- list tools
- inspect a tool
- validate a call
- execute a call
- return results [3]

That matches the MCP model itself, where tools are the callable functions exposed to the LLM or host [1]. In other words, the UI should mostly be a presentation layer over the server’s tool registry and call results.

## 6) Practical Rust stack implied by the sources
A reasonable Rust implementation stack looks like this:

- **Official MCP Rust SDK** for protocol support [9]
- **Tokio** for async runtime, especially if you need concurrent tool calls or HTTP handlers [2]
- **Serde / serde_json** for request/response serialization in JSON-RPC flows [2]
- **Axum** if you are using the embedded UI routes or building an HTTP server around the UI [3]
- A UI-specific crate such as **mcp_embedded_ui** for a built-in explorer [3]
- Or the **PMCP `mcp-apps` feature** if you want interactive widgets and host-integrated apps [5]

## 7) Recommended implementation strategy
If your goal is “mcp-ui in Rust,” the cleanest progression is:

1. **Build the MCP server first** using the Rust SDK [9].
2. Expose a few tools and verify they work over the chosen transport [1][2].
3. Add **embedded UI routes** with `mcp_embedded_ui` if you want a quick web explorer for tools [3].
4. If you need richer interactivity, move to **MCP Apps** and serve widget metadata plus self-contained HTML widgets [5].
5. Make sure logging stays off stdout for stdio transports [1].

## 8) Bottom line
The sources suggest that “

[truncated 600 chars]

## Evidence
| # | Source | Reason | Score |
| ---: | --- | --- | ---: |
| 1 | https://github.com/pulseengine/mcp/blob/main/docs/superpowers/specs/2026-03-28-rmcp-migration-phase0-phase1-design.md#L1-L60 | query result: https://github.com/pulseengine/mcp/blob/main/docs/superpowers/specs/2026-03-28-rmcp-migration-phase0-phase1-design.md#L1-L60 | 150 |
| 2 | https://github.com/modelcontextprotocol/rust-sdk | search result: GitHub - modelcontextprotocol/rust-sdk | 145 |
| 3 | https://modelcontextprotocol.io/docs/develop/build-server | search result: Build an MCP server - Model Context Protocol | 120 |
| 4 | https://github.com/RustSandbox/MCP-Development-with-Rust/blob/main/mcp_rust_tutorial.md | search result: MCP-Development-with-Rust/mcp_rust_tutorial.md at main - GitHub | 95 |

## Source Summaries
### 1. https://github.com/pulseengine/mcp/blob/main/docs/superpowers/specs/2026-03-28-rmcp-migration-phase0-phase1-design.md#L1-L60

- The document lays out the first two steps of a migration from PulseEngine’s custom MCP stack to the official Rust SDK `rmcp`.
- Phase 0 is a proof-of-concept phase with three small workspaces: Tower auth middleware, resource URI routing with `matchit`, and serving HTML/UI resources via an MCP Apps-style extension. Each PoC has clear success criteria and acts as a gate before migration continues.
- Phase 1 extracts already-generic crates into standalone packages with new names: `pulseengine-logging` and `pulseengine-security`. These mostly need Cargo, docs, and README renaming, with little or no code change.
- The spec explicitly defers deprecation of the old `pulseengine-mcp-*` crates until a later phase, and lists Phase 2–4 as out of scope here.
- It also calls out key risks, such as whether rmcp exposes HTTP request extensions, whether `matchit` can handle MCP URI templates, and whet

[truncated 40 chars]

### 2. https://github.com/modelcontextprotocol/rust-sdk

- This GitHub repository is the official Rust SDK for the Model Context Protocol (MCP), called **RMCP**, built on **tokio** for async Rust.
- It contains two main crates: **rmcp** (core protocol implementation) and **rmcp-macros** (procedural macros for tools, prompts, and handlers).
- The README highlights support for MCP features such as **tools, resources, prompts, completions, notifications, subscriptions, tasks, and OAuth**, with code examples for both client and server setups.
- Some features like **sampling, roots, and logging** are marked **deprecated** but still functional for now.
- The repo also includes examples, migration guidance for 1.x, development docs, and a list of related MCP projects built with RMCP.

### 3. https://modelcontextprotocol.io/docs/develop/build-server

- The page is a multi-language quickstart for building a simple **Model Context Protocol (MCP) weather server** that exposes two tools: **`get_alerts`** and **`get_forecast`**, then connects it to an MCP host like **Claude for Desktop**.
- It explains core MCP concepts—**resources, tools, and prompts**—and emphasizes that this tutorial focuses on **tools**. It also notes the server can be used by different clients, not just Claude.
- Each language section shows how to set up the project, install dependencies, implement the weather API calls to **api.weather.gov**, and register the tools:
  - **Python** uses `FastMCP` and `httpx`
  - **TypeScript** uses the MCP SDK, `zod`, and `fetch`
  - **Java/Spring AI** uses Spring Boot auto-configuration and `@Tool`
  - **Kotlin** uses the Kotlin SDK and Ktor
  - **C#** uses the .NET MCP SDK and hosting
- A recurring best practice is to avoid writing

[truncated 326 chars]

### 4. https://github.com/RustSandbox/MCP-Development-with-Rust/blob/main/mcp_rust_tutorial.md

- This GitHub page is a long tutorial, “Model Context Protocol (MCP) in Rust: Complete Tutorial,” for building MCP servers in Rust from beginner to enterprise level.
- It explains MCP basics: tools, resources, prompts, and sampling, and why Rust is a good fit because of memory safety, performance, concurrency, and type safety.
- The early examples cover simple servers like a hello-world greeting tool, a calculator with custom error handling, a text processor with multiple tools, a weather service, and a resource provider for documents.
- Later sections move into intermediate topics such as configurable servers using files, environment variables, and CLI arguments, plus validation, feature flags, and runtime configuration.
- The tutorial includes many Rust learning notes and code snippets, emphasizing serde JSON handling, async with Tokio, iterators, pattern matching, ownership, and custo

[truncated 179 chars]

## Gaps And Risks
- Verify code examples against the current crate versions before copying into production.

## Follow-Up Calls
- search/query: implementing mcp-ui in rust Rust rmcp server with MCP Apps UI resources, concrete APIs, metadata keys, MIME types, and implementation steps
- search/query: implementing mcp-ui in rust official docs examples
- search/query: implementing mcp-ui in rust API reference concrete code
- search/query: implementing mcp-ui in rust compatibility caveats

## Follow-Up Code Mode Snippet
```js
async () => {
  const input = {
  "topic": "implementing mcp-ui in rust",
  "focus": "Rust rmcp server with MCP Apps UI resources, concrete APIs, metadata keys, MIME types, and implementation steps",
  "queries": [
    "implementing mcp-ui in rust Rust rmcp server with MCP Apps UI resources, concrete APIs, metadata keys, MIME types, and implementation steps",
    "implementing mcp-ui in rust official docs examples",
    "implementing mcp-ui in rust API reference concrete code",
    "implementing mcp-ui in rust compatibility caveats"
  ],
  "seedUrls": [
    "https://github.com/pulseengine/mcp/blob/main/docs/superpowers/specs/2026-03-28-rmcp-migration-phase0-phase1-design.md#L1-L60",
    "https://github.com/modelcontextprotocol/rust-sdk",
    "https://modelcontextprotocol.io/docs/develop/build-server",
    "https://github.com/RustSandbox/MCP-Development-with-Rust/blob/main/mcp_rust_tutorial.md",
    "https://mcpui.dev/",
    "https://github.com/RustSandbox/MCP-Development-with-Rust",
    "https://docs.rs/mcp-embedded-ui/latest/mcp_embedded_ui/index.html?search=",
    "https://docs.rs/mcp-embedded-ui/latest/mcp_embedded_ui/all.html?search="
  ],
  "maxEvidenceUrls": 5
};

  const axon = (args) => callTool("axon::axon", args);
  const parseTool = (result) => {
    const text = result?.content?.[0]?.text;
    if (typeof text !== "string") return result;
    try { return JSON.parse(text); } catch { return { raw: text }; }
  };
  const timed = async (label, args) => {
    const started = Date.now();
    try {
      const result = await axon(args);
      const parsed = parseTool(result);
      return {
        label,
        ok: parsed?.ok ?? !result?.isError,
        ms: Date.now() - started,
        args,
        key

[truncated 5174 chars]
