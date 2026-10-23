# labby-codemode

Host-neutral Code Mode kernel for running JavaScript against a host-provided
capability catalog. The crate owns the Javy/QuickJS runner, its broker protocol,
discovery helpers, snippets, artifacts, error contracts, and result shaping.
The product gateway implements the host seam; this crate does not own upstream
connections, OAuth lifecycle, MCP registration, or HTTP routing.

This internal workspace crate (`publish = false`) supplies the kernel used by
the product's Code Mode surfaces.

## Source map

| Area | Owning source |
| --- | --- |
| Public exports and host contract | [lib.rs](src/lib.rs), [host.rs](src/host.rs) |
| Execution, scope checks, and discovery proxy | [execute.rs](src/execute.rs), [discovery.rs](src/discovery.rs), [preamble.rs](src/preamble.rs) |
| Process lifecycle and protocol | [runner.rs](src/runner.rs), [runner_drive.rs](src/runner_drive.rs), [protocol.rs](src/protocol.rs), [pool.rs](src/pool.rs) |
| Errors and final response | [error_contract.rs](src/error_contract.rs), [types.rs](src/types.rs), [shape.rs](src/shape.rs), [truncate.rs](src/truncate.rs) |
| Saved workflows and output files | [snippet](src/snippet.rs), [artifacts.rs](src/artifacts.rs), [artifact access](src/artifact_access.rs) |
| Reserved providers | [local_provider.rs](src/local_provider.rs), [state](src/state.rs), [git](src/git.rs), [sandbox](src/sandbox.rs) |
| Product adapter | [gateway CodeModeHost implementation](../labby-gateway/src/gateway/code_mode/code_mode_host.rs) |

## Public API and host responsibilities

`CodeModeHost: Send + Sync` supplies the catalog and dispatch policy. Tools have
opaque `<namespace>::<tool>` IDs and JSON parameters; `CatalogDescriptor` carries
their discovery metadata. `ToolsRender` contains shared descriptor and JSON
snapshots, fingerprints, and per-run summaries of withheld tools.

Required host methods provide `list_tools`, `call_tool`, `resolve_snippet`,
`semantic_rank`, `config`, `runner_pool`, `openapi_registry`, and
`openapi_http_client`. A host without semantic ranking returns an empty ranking;
a host without OpenAPI specs returns an empty registry and still supplies the
hardened dispatch client. The host preserves caller authorization, live tool
policy, route restrictions, and credential ownership at each dispatch.

Optional hooks cover resources, prompts, Skills, query-driven artifact search,
caller-bound OpenAPI credentials, and step/local-provider recording. Resource,
prompt, and Skill defaults reject unsupported operations. Artifact search
defaults to an empty result. Step decisions default to `Execute`, and recording
defaults to a no-op. None of these defaults creates a persistence service.

`CodeModeBroker::new(Some(host))` creates the broker for one execution. Use a
fresh broker per request: it retains that run's discovery snapshot and captured
widget metadata. `execute` returns the model-facing response;
`execute_with_trace_context` adds a host-owned request trace, and
`execute_with_raw_response` retains both raw and display responses.

`ExecCtx` carries protocol sequence, execution identity, call ordinal, step
ordinal, and optional trace context into host hooks. `CodeModeCaller` and
`CodeModeSurface` describe the initiating authority and transport; they do not
establish identity by themselves. The outer adapter must authenticate callers
and gate whether its Code Mode surface is enabled before invoking the broker.

The crate also exports bounded discovery functions, schema validation,
`normalize_user_code`, namespace resolution helpers, history/source stores,
trace redaction, storage-limit defaults, and receipted artifact reads. Follow
[lib.rs](src/lib.rs) for the complete export list.

## Integration shape

A host owns its `RunnerPool`, implements the required methods, installs its
configuration defaults, and creates a broker for each authorized request.
This example assumes an existing host implementation and an explicitly trusted
local CLI caller; network callers need their authenticated caller and scope.

```rust
use labby_codemode::{
    CodeModeBroker, CodeModeCaller, CodeModeExecutionError,
    CodeModeExecutionResponse, CodeModeHost, CodeModeSurface, ToolScope,
};

async fn discover<H: CodeModeHost>(
    host: &H,
) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
    CodeModeBroker::new(Some(host))
        .execute(
            "async () => { return await codemode.search('status'); }",
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            host.config().await,
            ToolScope::default(),
            None,
        )
        .await
}
```

The executable must handle `internal code-mode-runner` before normal application
startup and return `run_code_mode_runner_stdio()`'s `ExitCode`. The
[snippet harness executable](examples/snippet_harness.rs) demonstrates this
branch and a Tokio runtime. The library alone does not install that subcommand.

`RunnerPool::from_env()` defaults to re-executing the current binary with those
arguments. `LABBY_CODE_MODE_RUNNER_EXE` selects an absolute, validated override;
Unix validation rejects group/world-writable files and owners other than the
current user or root. A separate executable can instead be supplied through
`RunnerSpawn { program, args }` and `RunnerPool::with_spawn`; that explicit seam
uses the direct-process transport. The selected program must implement the same
protocol. Shut down the host-owned pool with `RunnerPool::shutdown().await`.

## Runner lifecycle and broker protocol

The pool reuses OS processes; every `Start` builds a fresh `javy::Runtime` and
resets sequence state. Never retain a JS runtime between callers. The runner
also attempts to replace its working directory with a fresh jail subdirectory.
That reset is best-effort: filesystem failures are non-fatal and can leave state
in the isolated runner directory. It is defense-in-depth, not a hard containment
boundary. Clean completion and ordinary execution errors return the process to
the pool. Timeouts and protocol faults evict it. A pooled process that exits
before any valid protocol activity gets one attempt on a fresh runner; activity
followed by a crash does not trigger automatic replay.

Pool defaults are two slots, recycle after 100 executions, and eight overflow
runners. `LABBY_CODE_MODE_POOL_SIZE` (maximum 16),
`LABBY_CODE_MODE_POOL_RECYCLE_AFTER`, and
`LABBY_CODE_MODE_POOL_MAX_OVERFLOW` (effective range 1–64) control them. Size zero disables
pooling while preserving bounded ephemeral execution. A broker without a host
uses the standalone runner path rather than a host pool.

[protocol.rs](src/protocol.rs) owns newline-delimited JSON messages tagged with
`type` and names such as `start` and `tool_call`. Parent input includes `Start`,
`ToolResult`, `ToolError`, `SnippetResolved`, `StepDecision`, and `StepRecorded`.
Runner output includes `ToolCall`, `ArtifactWrite`, `SnippetResolve`,
`StepBegin`, `StepResult`, `Done`, and `Error`. Requests and replies share `seq`;
step results reuse their begin sequence. Stdout is protocol-only. The runner
accepts another start after completion and exits on stdin EOF.

`codemode.step(name, fn)` awaits the host decision, executes the function, and
waits for recording acceptance before returning. `StepRecorded` means the hook
accepted the value. The gateway buffers bounded, redacted records in memory and
flushes them at the run boundary; the acknowledgement does not prove a durable
per-step write. The gateway uses the default execute decision. Replay variants
are reserved seams, with no caller pause, confirmation, or resume facility.

## Scope and discovery

`ToolScope` combines namespace and tool filters with optional read-only access.
`ToolScope::new([], [])` is unscoped; `scoped_namespaces([], [])` denies all
namespaces. Tool entries are filtered with `scope.allows` before proxy creation
and checked again for dispatch. A read-only scope also requires the host to
verify the live descriptor at invocation, beyond catalog filtering.

Hosts that recompute discovery visibility must use `discovery_render_params`
and `discovery_entry_visible`. Metadata entries are authorized by their owning
source before rendering; catalog membership never grants retrieval or loading.
The vocabulary includes tools, snippets, resources, prompts, Skills, commands,
and subagents; support for each family depends on the host adapter.

`codemode.search` matches the injected index and can request host semantic or
artifact search. `codemode.describe` resolves against that same index, then
fetches tool declarations from the retained render through a reserved internal
bridge. Missing declarations produce `schema_status: "unavailable"` and
`schema_error`. Internal handlers that read entries must reapply visibility;
the host cache alone is not a per-tool authorization boundary.

JS calls use generated `codemode.<namespace>.<tool>(params)` helpers or
`callTool("namespace::tool", params)`. Resource, prompt, and Skill helpers call
their separate host hooks. `codemode.batch` settles independent jobs into `ok`
and `failed` collections with input indices and structured errors; it does not
remove the need to bound fan-out, response size, and total calls.

## Results, errors, snippets, and artifacts

`CodeModeExecutionResponse` carries final `result`, call metadata, bounded logs,
artifact receipts, and optional widget/shaping metadata. JavaScript `undefined`
omits `result`; explicit `null` remains JSON null. Call traces hold timings,
status, redacted parameters, and error kinds rather than full tool outputs.
Final shaping and envelope truncation apply after execution; configured shaping
does not alter the values that JS receives from `callTool`.

`CodeModeCallError` preserves origin, evidence, recovery, and side-effect risk
across the bridge. `CodeModeExecutionError` adds the calls accumulated before
failure. Read `calls()` before converting it; `into_contract_tool_error()` keeps
the refined contract for outer adapters. Follow the
[tool-error contract](../../docs/contracts/code-mode-tool-errors.md) when
mapping failures. A timeout does not establish whether an upstream write ran.

The host returns optional `UiLink` metadata with a tool outcome. The broker
retains the last widget link for the response, and `{ __ui: value }` remains a
compatibility return wrapper that unwraps `value`. Rendering `_meta.ui` and
advertising the optional inspector belong to the product surface.

`codemode.run(name, input)` resolves source through the host and runs it in the
same JS runtime and scope. The public `snippet::store` API handles Markdown/JS
sources, inputs, declarations, digests, validation, and atomic publication;
`snippet::harness` runs offline fixtures. The store's `ResolvedSnippet` includes
source metadata, while the host-level type contains executable code and merged
input. These types serve different stages of resolution.

`writeArtifact(path, content, options)` brokers text to the host artifact store.
Writes reject traversal and existing symlink ancestors and return size/digest
receipts. Oversized results can also produce recoverable output artifacts.
Artifact visibility follows caller/scope policy. `read_receipted_artifact`
validates retained bytes against receipt identity, size, and SHA-256; the host
must authorize the execution before calling it. Storage uses Labby's home
directory; configure its location and retention through the owning runtime.

## Reserved providers and authority

`state`, `git`, `openapi`, and `sandbox` are reserved local namespaces, intercepted
before host tool dispatch. Their methods cannot route to a same-named upstream.

| Provider | Boundary and prerequisites |
| --- | --- |
| `state` | Unscoped admin/trusted local authority; virtual paths and quotas within the host's Code Mode workspace. Shares the local-provider lock with `git`. |
| `git` | Same authority and workspace; invokes the host Git executable with constrained commands, paths, and remote policy. Requires Git on the host. |
| `openapi` | Uses `labby-openapi` registry and hardened HTTPS client outside that lock. Static/no-auth operations require unscoped admin/trusted authority. Subject-scoped OAuth operations can admit authenticated, unscoped execution callers with a verified subject; the host resolves credentials after the sandbox boundary. |
| `sandbox` | Unscoped admin/trusted authority plus the optional `microsandbox-sdk` feature and operator-owned `LABBY_CODE_MODE_SANDBOX_PROFILE_JSON`. Runs disposable workloads through the local SDK, outside the state/Git lock. Without the feature, dispatch reports `not_available`. |

Git uses `/usr/bin/git` on Unix by default. For systems such as Nix where Git
lives elsewhere, set operator-owned `LABBY_CODE_MODE_GIT_EXE` to an absolute
executable path. Labby resolves symlinks and, on Unix, requires a root- or
service-user-owned executable that is not group/world writable. The override
does not inherit the caller environment or extend the sanitized child `PATH`.

Any namespace, tool, or read-only scope excludes these local providers, including
the OpenAPI OAuth exception. Workload requests cannot grant mounts, networking,
or credentials. They must match the pinned profile image; their command and
projected text files are bounded. The profile caps CPU at 1–2, memory at
128–1024 MiB, and workload time at 15 seconds. The SDK needs an installed local
Microsandbox runtime and cached image; it does not auto-install them. The
product enables this crate feature through `codemode-microsandbox` in its
[manifest](../labby/Cargo.toml).

## Limits and platform requirements

| Limit | Default or hard boundary |
| --- | --- |
| Source / JS heap / native stack | 1 MiB source ceiling; 64 MiB heap and 256 KiB stack per fresh runtime. |
| Wall time | `CodeModeConfig.timeout_ms` defaults to 30 seconds; validated range is 1–300,000 ms. Response/settlement reserves share that deadline. |
| Calls / one tool result | 512 ordinary calls, configurable up to 2,048; excess calls reject as `call_budget_exceeded`. One tool result defaults to 8 MiB before truncation. |
| Internal bridges / snippet resolution | 32 internal requests; 32 snippet resolutions and 1 MiB total resolved source per run, plus recursion/depth checks. |
| Step records | 256 steps, 256 KiB per serialized value, and 8 MiB of values per run. |
| Response / logs | 24 KiB and approximately 6,000 response tokens; 1,000 log entries and 64 KiB logs. |
| Artifact content / retention | 8 MiB per artifact; retention targets 200 run directories and 4,096 MiB total store. Pruning is best-effort and excludes active runs. |

The host installs process-level fallbacks with
`install_call_budget_config_defaults` and `install_artifact_config_defaults`;
environment overrides take precedence. `effective_storage_limits()` reports
resolved values. See [configuration](../../docs/runtime/CONFIG.md) for settings.

Use the repository [Rust toolchain](../../rust-toolchain.toml) and workspace
build prerequisites. The default backend is a self-reexecuted process with
Javy/QuickJS linked into the binary, cleared environment, an isolated cwd,
kill-on-drop, Unix process groups, and Windows Job Object containment. QuickJS
has no ambient fetch, Node modules, or direct host filesystem API. Linux also attempts
to disable process dumps. Tool authority and credentials stay in the parent.

The separate Linux-only runner transport is enabled with
`LABBY_CODE_MODE_RUNNER_BACKEND=microsandbox`. It requires validated absolute
`LABBY_CODE_MODE_MICROSANDBOX_EXE` and a cached immutable SHA-256 image in
`LABBY_CODE_MODE_MICROSANDBOX_IMAGE`. It attaches with `msb exec --stream`, mounts
the validated runner read-only, disables networking, and uses 1 CPU/256 MiB.
This transport does not require the `microsandbox-sdk` workload feature.

## Focused development checks

Run from the repository root in its supported environment:

```bash
cargo check -p labby-codemode --all-targets
cargo nextest run -p labby-codemode --all-features
cargo run -p labby-codemode --example snippet_harness -- --schema
```

The harness is a real QuickJS subprocess with synthetic tool responses. It does
not validate live upstream schemas, credentials, or a real microVM. Ordinary
tests and fake `msb` lifecycle fixtures likewise do not qualify a real
Microsandbox installation. Explicitly ignored SDK live tests need their cached
image, operator profile, and working runtime. Follow [testing](../../docs/dev/TESTING.md),
[snippet testing](../../docs/dev/SNIPPET_TESTING.md), and the
[crate instructions](AGENTS.md) for the relevant checks and invariants.

For product behavior and examples, use [Code Mode](../../docs/dev/CODE_MODE.md)
and [Snippets](../../docs/services/SNIPPETS.md). Read
[architecture](../../docs/ARCH.md) for the host boundary and
[documentation maintenance](../../docs/dev/DOCUMENTATION.md) before changing
generated catalogs or other documentation owners.

## Discovery implementation history

A prior investigation (bead `lab-5cgrz`) evaluated converting `search`/`describe`
to host-RPC via the `local_provider.rs` pattern specifically and rejected it: the
claimed injection cost was negligible at then-current scale, that approach would
have serialized repeated calls behind the global local-provider lock, and reusing
`local_providers_allowed()` verbatim would have wrongly restricted them to
admin-only. `describe()` alone was later revisited once catalog sizes grew large
enough for the injection cost to matter (informally benchmarked with an ad-hoc
`#[ignore]`d probe against a real `javy::Runtime`, not a checked-in regression
test: roughly 3x smaller injected payload, roughly 1.8x faster parse at 4,000
tools — re-measure before treating these numbers as current) — solving the
concerns the bead flagged as prerequisites: the
`__lab_internal::*` mechanism sidesteps both the lock-serialization and
admin-only-gating objections by construction (it's a different mechanism, not a
fix to the rejected one), and `ToolsRender`/`CatalogRenderCache`
(`labby-codemode`/`labby-gateway`) Arc-wrap `entries`/`catalog_json`. The
broker now retains that render for the execution, removing the old second
`list_tools()` call from the normal `describe()` path entirely. `search()` was
left alone — its injection cost (index metadata only, never full types) is a
fixed, small fraction of what `describe()` used to embed, so the bead's original
"negligible at scale" conclusion still holds for it.
