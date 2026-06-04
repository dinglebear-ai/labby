---
name: acp
version: "1.0.0"
description: >-
  This skill should be used when implementing an ACP agent or extending the lab ACP runtime in Rust —
  including using Client.builder()/ByteStreams/attach_session to connect to ACP providers, calling
  session_config_options() to discover models and config options, switching models via
  SetSessionConfigOptionRequest, implementing the Agent trait for a new ACP provider (stdio agent),
  handling session/prompt or session/update wire messages, or debugging JSON-RPC 2.0 stdio transport
  issues. Also applies when working on crates/lab/src/acp/runtime.rs, the codex-acp reference
  implementation, or authoring bidirectional stdio agents for Zed or VS Code.
---

# Agent Client Protocol (ACP) — Rust

ACP is a JSON-RPC 2.0 protocol for bidirectional communication between AI coding agents and editor clients (Zed, VS Code, etc.). Agents run as subprocesses — clients write to stdin, read from stdout. stderr is for logs only, never protocol data.

**Lab version pin:** `agent-client-protocol = { version = "=0.13.1", features = ["unstable"] }` in `crates/lab/Cargo.toml`. When upgrading, pin to an exact version, verify the `unstable` feature still compiles, and re-check `session_config_options()` behavior against the new `SessionConfigOption` / `SessionConfigKind::Select` API.

**Two roles in this codebase:**
- **Client (lab runtime):** lab is the client; providers (codex-acp, etc.) are the agents. The lab runtime uses `Client.builder()` + `ByteStreams` + `attach_session`. See "Lab ACP Runtime" below.
- **Agent (providers):** codex-acp and custom providers implement the `Agent` trait and run on stdio. Lab spawns them as subprocesses. See "Implementing an Agent" below.

**SDK source:** `~/workspace/acp/rust-sdk/` — canonical trait signatures  
**Production reference:** `~/workspace/acp/codex-acp/` (Rust agent for OpenAI/Codex)  
**Schema types:** `~/workspace/acp/agent-client-protocol/` — schema crate only (`InitializeRequest`, `AuthMethod`, etc.). Does **not** contain `Agent`/`Client` traits or the runtime layer.

---

## Cargo.toml

**Lab binary (pinned, unstable features — use this in crates/lab):**
```toml
agent-client-protocol = { version = "=0.13.1", features = ["unstable"] }
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["compat"] }  # required: .compat() / .compat_write() bridge
```

**Standalone ACP agent (new provider binary):**
```toml
agent-client-protocol = "0"               # types + transport (AgentSideConnection, Agent trait)
async-trait = "0.1"                       # required: Agent trait needs #[async_trait(?Send)]
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["compat"] }
futures = "0.3"                           # AsyncRead/AsyncWrite traits expected by AgentSideConnection
anyhow = "1"
uuid = { version = "1", features = ["v4"] }
dashmap = "5"   # preferred over std::sync::Mutex<HashMap> in async contexts
```

---

## Session Lifecycle (condensed)

```
initialize  →  authenticate  →  session/new  →  session/prompt (streaming)  →  session/cancel
```

All streaming happens via `session/update` notifications sent **from agent to client** during prompt execution. The final `PromptResponse` matches the original `session/prompt` request id.

See `references/wire-format.md` for full JSON examples of every message.

---

## Lab ACP Runtime — Client-Side API

The lab runtime (`crates/lab/src/acp/runtime.rs`) uses the builder API. Lab is the **client**; ACP providers (codex-acp etc.) are **agents** spawned as subprocesses.

### Connection and Session Start

```rust
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, on_receive_request};
use agent_client_protocol::schema::{
    InitializeRequest, NewSessionRequest, ProtocolVersion, Implementation,
    SetSessionConfigOptionRequest, SessionConfigOption, SessionConfigKind, SessionConfigSelectOptions,
};

// Wrap subprocess stdio in ByteStreams — requires .compat_write() / .compat() bridges.
let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());

// Client.builder() registers inbound request handlers, then connect_with drives the session.
// on_receive_request!() is the required second argument — a macro-generated registration handle.
Client
    .builder()
    .on_receive_request(
        async move |args: RequestPermissionRequest, responder, _cx| {
            responder.respond(handle_permission(args).await)
        },
        on_receive_request!(),
    )
    .connect_with(transport, move |connection: ConnectionTo<Agent>| async move {
        // 1. Initialize — protocol-level, use send_request (NOT send_request_to)
        let initialized = connection
            .send_request(
                InitializeRequest::new(ProtocolVersion::V1)
                    .client_info(Implementation::new("lab-acp-bridge", env!("CARGO_PKG_VERSION")))
                    .client_capabilities(lab_client_capabilities()),
            )
            .block_task()
            .await?;

        // 2. Create session — use send_request_to(Agent, ...) NOT send_request(...)
        //    NewSessionRequest::new(&*cwd) — cwd: String, deref to str
        let new_session_response = connection
            .send_request_to(Agent, NewSessionRequest::new(&*cwd))
            .block_task()
            .await?;

        // 3. Read config options BEFORE attach_session — it consumes the response
        let (model_id, models) = session_config_options(
            new_session_response.config_options.as_deref().unwrap_or_default(),
        );

        // 4. Attach session — produces the session handle for read_update() and further requests
        let mut session = connection
            .attach_session(new_session_response, vec![])
            .map_err(|e| acp_internal_error(e.to_string()))?;

        // session.session_id()   — provider session ID
        // session.read_update()  — await next SessionMessage from provider
        // session.connection()   — get connection back for further send_request_to() calls

        Ok::<(), agent_client_protocol::Error>(())
    })
    .await;
```

> **GOTCHA — `send_request_to(Agent, ...)` vs `send_request(...)`:** Session-scoped requests (`NewSessionRequest`, `SetSessionConfigOptionRequest`, `PromptRequest`) must use `.send_request_to(Agent, req)`. Plain `.send_request(req)` is for protocol-level messages only (`InitializeRequest`).

> **GOTCHA — read config_options before attach_session:** `attach_session` consumes the `NewSessionResponse`. Always extract `config_options` from it first.

### session_config_options() — Model and Config Discovery

```rust
fn session_config_options(raw: &[SessionConfigOption]) -> (Option<String>, Vec<ModelOption>) {
    let mut model_id = None;
    let mut models = Vec::new();

    for opt in raw {
        let is_model = opt.category.as_ref() == Some(&SessionConfigOptionCategory::Model);
        if let SessionConfigKind::Select(select) = &opt.kind {
            let current = select.current_value.to_string();
            let opts: Vec<ModelOption> = match &select.options {
                SessionConfigSelectOptions::Ungrouped(options) => options.iter()
                    .map(|o| ModelOption { id: o.value.to_string(), name: o.name.clone() })
                    .collect(),
                SessionConfigSelectOptions::Grouped(groups) => groups.iter()
                    .flat_map(|g| g.options.iter()
                        .map(|o| ModelOption { id: o.value.to_string(), name: o.name.clone() }))
                    .collect(),
                _ => Vec::new(),
            };
            if is_model { model_id = Some(current); models = opts; }
        }
    }
    (model_id, models)
}
```

### Model Switching

Send `SetSessionConfigOptionRequest` before the next prompt turn:

```rust
session
    .connection()
    .send_request_to(
        Agent,
        SetSessionConfigOptionRequest::new(session.session_id().clone(), "model", model_id),
    )
    .block_task()
    .await?;
```

### Prompt Loop Pattern

```rust
// Start prompt asynchronously; StopReason arrives via on_receiving_result callback.
session
    .connection()
    .send_request_to(Agent, PromptRequest::new(session.session_id().clone(), content_blocks))
    .on_receiving_result(async move |result| {
        drop(prompt_response_tx.send(result.map(|r| r.stop_reason).map_err(|e| e.to_string())));
        Ok(())
    })
    .map_err(|e| acp_internal_error(e.to_string()))?;

// biased select! ensures StopReason is preferred over a simultaneous idle timeout.
loop {
    tokio::select! {
        biased;
        stop = &mut prompt_response_rx => { /* handle StopReason, break */ }
        update = session.read_update() => { /* handle SessionMessage */ }
        () = tokio::time::sleep(idle_timeout), if saw_assistant_output => { /* idle_completion, break */ }
    }
}
```

### Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS` | 5000 | Idle timeout after last assistant chunk |
| `LAB_ACP_TURN_DRAIN_TIMEOUT_MS` | 300000 | Max wait draining late StopReason after idle_completion |
| `LAB_ACP_PERMISSION_TIMEOUT_MS` | 60000 | Permission request decision window |

---

## Implementing an Agent

Implement the `Agent` trait to build an ACP provider that runs on stdio:

```rust
// The Agent trait uses ?Send bounds. In the =0.13.x SDK, implement with
// #[async_trait::async_trait(?Send)] — native async fn in trait does NOT work here.
#[async_trait::async_trait(?Send)]
impl Agent for MyAgent {
    async fn initialize(&self, req: InitializeRequest) -> acp::Result<InitializeResponse>;
    async fn authenticate(&self, req: AuthenticateRequest) -> acp::Result<AuthenticateResponse>;
    async fn new_session(&self, req: NewSessionRequest) -> acp::Result<NewSessionResponse>;

    // prompt() takes ONLY PromptRequest — there is NO SessionNotifier parameter.
    // Streaming updates are sent via conn.session_notification() from a background task.
    // See "Streaming Notifications" section below for the required mpsc channel pattern.
    async fn prompt(&self, req: PromptRequest) -> acp::Result<PromptResponse>;

    // Method name is cancel (NOT on_cancel). Returns Result<()>.
    async fn cancel(&self, notification: CancelNotification) -> acp::Result<()>;

    // Optional methods (default: Err(Error::method_not_found())):
    //   load_session, set_session_mode, set_session_config_option, list_sessions
    // UNSTABLE (behind feature flags): close_session, fork_session, resume_session, set_session_model
}

// Entry point — use current_thread flavor (?Send trait requires LocalSet).
// MUST use .compat() / .compat_write() — AgentSideConnection expects futures::AsyncRead/AsyncWrite,
// NOT tokio::io traits. These are different trait families.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel::<NotifMsg>();
    let agent = Arc::new(MyAgent { notif_tx, sessions: Arc::new(DashMap::new()) });

    tokio::task::LocalSet::new().run_until(async move {
        // conn implements Client — use it to call session_notification, request_permission, etc.
        // io_task drives the stdio read/write loop.
        let (conn, io_task) = AgentSideConnection::new(
            agent,
            tokio::io::stdout().compat_write(), // outgoing
            tokio::io::stdin().compat(),         // incoming
            |fut| { tokio::task::spawn_local(fut); },
        );

        // Background task: receive (notification, done_tx) from agent, send via conn.
        tokio::task::spawn_local(async move {
            while let Some((notif, done_tx)) = notif_rx.recv().await {
                if conn.session_notification(notif).await.is_err() { break; }
                let _ = done_tx.send(());
            }
        });

        io_task.await
    }).await
}
```

> **GOTCHA — no SessionNotifier in prompt():** `SessionNotifier` does **not** exist in the SDK. `prompt()` receives only `PromptRequest`. Send streaming updates via `conn.session_notification()` called from a background task. The agent communicates with the background task via an mpsc channel stored in `self`.

> **GOTCHA — will not compile without compat:** `tokio::io::stdin()` does NOT implement `futures::AsyncRead`. Always use `.compat()` (read) and `.compat_write()` (write) from `tokio_util::compat`. Without `?Send` and `LocalSet`, the runtime panics on `!Send` types.

For a complete working skeleton see **`examples/agent-impl.rs`**.

Key points:
- Advertise only capabilities the agent actually supports in `InitializeResponse`
- Use `ProtocolVersion::V1` (not `LATEST`) in `InitializeResponse::new()`
- Return `Err(acp::Error::auth_required())` explicitly on auth failure (maps to JSON-RPC -32000)
- Use `tokio::io::stdin/stdout()` with `.compat()` — never `std::io` in an async context (blocks executor)
- Use `DashMap` for session state, not `std::sync::Mutex<HashMap>` (deadlock risk under Tokio)
- Add `#![deny(clippy::print_stdout, clippy::print_stderr)]` — one stray `println!` corrupts the binary protocol stream

---

## Streaming Notifications Pattern

The `prompt()` method has no access to the connection. To stream updates during a prompt turn, use an mpsc channel:

```rust
// In the agent struct:
type NotifMsg = (SessionNotification, tokio::sync::oneshot::Sender<()>);
struct MyAgent {
    notif_tx: tokio::sync::mpsc::UnboundedSender<NotifMsg>,
    sessions: Arc<DashMap<String, SessionState>>,
}

// Helper method for sending updates from prompt():
async fn send_update(&self, session_id: &str, update: SessionUpdate) -> acp::Result<()> {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let notif = SessionNotification::new(session_id.to_string(), update);
    self.notif_tx.send((notif, done_tx)).map_err(|_| acp::Error::internal_error())?;
    done_rx.await.map_err(|_| acp::Error::internal_error())
}

// In prompt() — use self.send_update() to stream:
async fn prompt(&self, req: PromptRequest) -> acp::Result<PromptResponse> {
    self.send_update(&req.session_id, SessionUpdate::AgentMessageChunk(
        ContentChunk::new("Thinking...".into())  // .into() converts &str → ContentBlock
    )).await?;
    Ok(PromptResponse::new(StopReason::EndTurn))
}

// In main() — background task owns conn, drains the channel:
tokio::task::spawn_local(async move {
    while let Some((notif, done_tx)) = notif_rx.recv().await {
        if conn.session_notification(notif).await.is_err() { break; }
        let _ = done_tx.send(());
    }
});
```

> **GOTCHA — ContentChunk::new takes ContentBlock:** `ContentChunk::new(content: ContentBlock)` — NOT a bare `&str`. Use `ContentChunk::new("text".into())` which works because `From<T: Into<String>> for ContentBlock` is implemented — `"text".into()` becomes `ContentBlock::Text(TextContent::new("text"))`. `ContentChunk::new("text")` is a compile error.

---

## Implementing a Client (generic)

For generic client implementations not using the lab runtime's `Client.builder()` pattern:

```rust
// The Client trait also requires #[async_trait::async_trait(?Send)] in this SDK version.
#[async_trait::async_trait(?Send)]
impl Client for MyClient {
    // REQUIRED: receives session/update notifications (streaming chunks, tool calls, etc.).
    async fn session_notification(&self, args: SessionNotification) -> acp::Result<()>;

    // REQUIRED: agent calls this before any destructive operation.
    // Returns RequestPermissionResponse (wraps outcome), NOT RequestPermissionOutcome directly.
    // Outcome: Cancelled | Selected(SelectedPermissionOutcome::new(option_id))
    async fn request_permission(&self, args: RequestPermissionRequest) -> acp::Result<RequestPermissionResponse>;

    // Optional (default: Err(method_not_found)) — only needed if you advertise fs capability:
    async fn read_text_file(&self, args: ReadTextFileRequest) -> acp::Result<ReadTextFileResponse>;
    async fn write_text_file(&self, args: WriteTextFileRequest) -> acp::Result<WriteTextFileResponse>;
    // Optional terminal methods: create_terminal, terminal_output, release_terminal,
    //                            wait_for_terminal_exit, kill_terminal
}

// Spawn agent subprocess and connect.
// Arg order: (client_handler, outgoing→agent_stdin, incoming←agent_stdout, spawner)
// conn implements Agent — call conn.initialize(), conn.prompt(), etc. to drive the session.
let (conn, io_task) = ClientSideConnection::new(
    MyClient,
    agent_stdin.compat_write(),  // outgoing
    agent_stdout.compat(),       // incoming
    |fut| { tokio::task::spawn_local(fut); },
);
// Drive session in a spawned task; await io_task to run until connection closes.
```

For a complete working skeleton see **`examples/client-impl.rs`**.

---

## Tool Calls (streaming)

Send `ToolCall` before executing a tool, then `ToolCallUpdate` with the result. Use `self.send_update()` from the streaming pattern above.

```rust
// Before tool execution — builder pattern, no Default impl
self.send_update(&req.session_id, SessionUpdate::ToolCall(
    ToolCall::new("tc-1", "Read src/main.rs")
        .kind(ToolKind::Read)
        .status(ToolCallStatus::InProgress)
        .locations(vec![ToolCallLocation::new("src/main.rs")]),
)).await?;

// After tool execution — ToolCallUpdateFields builder, #[serde(flatten)] in wire format
self.send_update(&req.session_id, SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
    "tc-1",
    ToolCallUpdateFields::new()
        .status(ToolCallStatus::Completed)
        .content(vec![ToolCallContent::Content(Content::new(
            ContentBlock::Text { text: result },
        ))]),
))).await?;
```

> **GOTCHA — no struct literals:** `ToolCall` and `ToolCallUpdate` have no `Default` impl. Use the builder pattern — `ToolCall::new(id, title).kind(...).status(...)`. `ToolCallStatus::Started` does **not** exist; use `InProgress`. The enum is `ToolKind` (not `ToolCallKind`).

For all 10 `ToolKind` variants, JSON wire format, streaming deduplication, and `_meta` extensibility see **`references/tool-calls.md`**.

---

## Reference Files

- **`references/wire-format.md`** — Full JSON-RPC examples for every message type. Reach for this when debugging wire format mismatches or building a client from scratch.
- **`references/message-reference.md`** — Complete table of all 24 ACP methods, all 11 `SessionUpdate` variants, session modes, and error codes.
- **`references/tool-calls.md`** — Tool call kinds table, full JSON wire examples, streaming deduplication pattern, `_meta` extensibility, terminal tool lifecycle.
- **`references/codex-patterns.md`** — Production patterns extracted from codex-acp: `OnceLock` global client, `SessionClient` error-tolerant notification wrapper, `DashMap` session state, `LocalSet` + compat wiring, filesystem sandboxing, auth guard, graceful cancellation.
- **`references/unstable-features.md`** — All 9 unstable feature flags with Cargo.toml activation syntax and stability tracking.

---

## Examples

- **`examples/agent-impl.rs`** — Complete `Agent` trait implementation skeleton with `DashMap` session state, mpsc notification channel, tool call notifications, and correct `tokio::io` usage.
- **`examples/client-impl.rs`** — Complete `Client` trait implementation skeleton with subprocess spawning, `session_notification` handler, file I/O handlers, and permission handling.

---

## Quick Checklists

### Extending the Lab ACP Runtime (crates/lab/src/acp/runtime.rs)

- [ ] Use `Client.builder().on_receive_request(..., on_receive_request!()).connect_with(transport, ...)` — not `ClientSideConnection`
- [ ] `transport = ByteStreams::new(stdin.compat_write(), stdout.compat())`
- [ ] Use `send_request_to(Agent, NewSessionRequest::new(&*cwd))` — not `send_request()`
- [ ] Extract `config_options` from `NewSessionResponse` **before** calling `attach_session` (which consumes it)
- [ ] Use `session_config_options()` to parse `SessionConfigKind::Select` into current model + available models
- [ ] Model switching: `send_request_to(Agent, SetSessionConfigOptionRequest::new(session_id, "model", model_id))`
- [ ] Use `biased` select! in the prompt loop — prevents idle timeout from winning over a simultaneous StopReason
- [ ] Spawn provider subprocess with `env_clear()` + explicit allowlist — never forward the full environment
- [ ] Use `process_group(0)` on Unix — enables SIGTERM to the entire process group on shutdown

### New Rust ACP Agent (standalone provider binary)

- [ ] `#![deny(clippy::print_stdout, clippy::print_stderr)]` in crate root — one stray `println!` corrupts the binary protocol stream
- [ ] Add `async-trait = "0.1"` to Cargo.toml — `Agent` trait requires `#[async_trait::async_trait(?Send)]` in this SDK version
- [ ] Run `AgentSideConnection` inside `tokio::task::LocalSet` — required for `!Send` types
- [ ] Use `#[tokio::main(flavor = "current_thread")]` — matches the `?Send` trait requirement
- [ ] Add `use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt}` — call `.compat()` / `.compat_write()` on tokio IO types (they do NOT implement `futures::AsyncRead/Write` natively)
- [ ] `AgentSideConnection::new` returns `(conn, io_task)` — **use `conn`** for `session_notification`; don't discard it
- [ ] Store an `mpsc::UnboundedSender<NotifMsg>` in the agent — this is how `prompt()` sends streaming updates
- [ ] Spawn a background task that drains the channel and calls `conn.session_notification()`
- [ ] `initialize` — advertise only capabilities the agent actually supports; use `ProtocolVersion::V1`
- [ ] `authenticate` — validate credentials; return `Err(acp::Error::auth_required())` on failure
- [ ] `new_session` — generate UUID, store state in `DashMap`; `req.cwd` is `PathBuf` (not `Option<PathBuf>`)
- [ ] `prompt` — only takes `PromptRequest` (no SessionNotifier!); use `send_update()` helper for streaming
- [ ] `cancel` (not `on_cancel`) — store a `watch::Sender<bool>` in session state, signal it; race with `biased tokio::select!` in prompt loop
- [ ] Keep stderr for logs only — never write protocol data to stderr
- [ ] Sandbox file paths to session `cwd` — reject `../` escapes using `std::path::absolute()`

### New Rust ACP Client (generic, not lab runtime)

- [ ] Add `async-trait = "0.1"` — `Client` trait requires `#[async_trait::async_trait(?Send)]`
- [ ] Spawn agent binary with `tokio::process::Command`, pipe stdio
- [ ] `ClientSideConnection::new` arg order: `(client, outgoing→agent_stdin, incoming←agent_stdout, spawner)`
- [ ] `ClientSideConnection::new` returns `(conn, io_task)` — use `conn.initialize()` etc. to drive the session
- [ ] Implement `session_notification` — **required**; route `SessionUpdate` variants to render in UI
- [ ] Implement `request_permission` — **required**; return `Cancelled` or `Selected(SelectedPermissionOutcome::new(option_id))`
- [ ] Implement `read_text_file`/`write_text_file` only if you advertise `fs` capability in `InitializeRequest`
- [ ] Handle all `SessionUpdate` variants (chunk, tool_call, tool_call_update, thought)
- [ ] Send `session/cancel` via `conn.cancel(CancelNotification::new(session_id))` on user interrupt
- [ ] Render tool calls using `kind` to pick appropriate UI (diff, file path, terminal)
- [ ] Gracefully degrade for capabilities the agent doesn't advertise
