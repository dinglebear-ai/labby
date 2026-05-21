# Chat Adapter Model Switching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add adapter-scoped model selection to the gateway-admin chat UI and ACP runtime so a selected model is valid for the active adapter, visible on existing sessions, and sent through the session/request path.

**Architecture:** Treat models as session/provider configuration, not as global UI state. The frontend derives model choices from the selected adapter/session and sends a model selection only when it is valid for that adapter. The Rust ACP bridge stores model/config-option state on session summaries and applies model changes through ACP `session/set_config_option` with category `model` when the provider exposes it, falling back to fixed/default model metadata when a provider has no switchable model selector.

**Tech Stack:** Rust 2024, Axum, `agent-client-protocol` with unstable ACP config APIs, serde, React 19, Next.js static export, TypeScript, node:test, Playwright.

---

## Research Notes

Relevant current repo facts:

- `apps/gateway-admin/components/chat/chat-input.tsx` owns the adapter picker UI and sends `ChatInputPayload` with `text` and `attachments` only.
- `apps/gateway-admin/lib/chat/chat-session-provider.tsx` owns selected provider/session state. It posts `POST /v1/acp/sessions` with `{ provider }` and `POST /v1/acp/sessions/{id}/prompt` with `{ prompt, attachments?, pageContext? }`.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` already creates a provider-matched session when the selected provider differs from the selected run.
- `apps/gateway-admin/lib/chat/acp-normalizers.ts` maps backend `RawSessionSummary` into `ACPRun`, but session model/config state is not represented.
- `crates/lab/src/api/services/acp.rs` exposes the browser ACP routes. `CreateSessionBody` accepts `provider`, `cwd`, and `title`; `PromptBody` accepts `prompt` and `page_context`.
- `crates/lab/src/dispatch/acp/dispatch.rs` routes `session.start` and `session.prompt`. Neither path accepts a model/config option today.
- `crates/lab/src/acp/registry.rs` creates sessions, persists `AcpSessionSummary`, and calls `RuntimeHandle::prompt(prompt)`.
- `crates/lab/src/acp/runtime.rs` launches the ACP provider, calls `start_session()`, and later calls `session.send_prompt(prompt)`.
- `lab_apis::acp::types::AcpSessionSummary` has provider/session metadata but no model/config fields.
- `agent-client-protocol-schema 0.12.0` already contains `SessionConfigOption`, `SessionConfigOptionCategory::Model`, `SetSessionConfigOptionRequest`, and `SetSessionConfigOptionResponse`. It also still has `SessionModelState` and `session/set_model`, but the ACP docs mark direct model selection as unstable and recommend session config options.

External protocol facts:

- ACP Session Config Options let agents expose arbitrary session selectors. The protocol recommends category metadata like `model`, but clients must not require categories for correctness.
- When a config option changes, the response returns the full config option state because one selection can affect other selectors.
- Agents must have defaults and run even if the client does not display or set config options.
- The TypeScript ACP SDK exposes `setSessionConfigOption()` as the stable config update call and `unstable_setSessionModel()` as experimental.

Scope decision from CEO review: HOLD SCOPE. Do not add a general settings framework, user preference persistence, non-chat model policy, provider install UI changes, or custom model discovery outside the ACP provider/session contract. The minimum shippable slice is: represent adapter model options, render a scoped selector, reset invalid model selections on adapter change, persist selected/current model on sessions, and send/apply the model before the next chat turn.

Engineering review synthesis:

- Architecture risk: if the UI stores a single `selectedModelId`, switching adapter can silently send an invalid cross-adapter model. Model selection must be keyed by provider and validated at send/create time.
- Simplicity risk: do not build a broad configuration framework. Add small model/config helpers that specifically extract the category `model` selector and keep the raw config options for future display.
- Security risk: model IDs are untrusted client input. Validate against the current provider/session options server-side before calling the ACP runtime. Log model IDs only as non-secret values; never include prompt text in model-change logs.
- Performance risk: provider/config option discovery should ride existing provider/session responses. Do not add polling or live discovery loops.

## File Structure

Modify these files:

- `crates/lab-apis/src/acp/types.rs`: add serializable model/config option DTOs to `AcpSessionSummary` and provider health if needed by the browser API.
- `crates/lab/src/acp/types.rs`: extend `StartSessionInput` and `StartSessionResult` with optional selected model/config options.
- `crates/lab/src/acp/runtime.rs`: capture model/config options from `start_session()`, send `session/set_config_option` before prompts when a selected model differs from current session config, and emit config update events.
- `crates/lab/src/acp/registry.rs`: persist selected/current model on session summaries, validate model selections, and pass selected model into runtime calls.
- `crates/lab/src/dispatch/acp/dispatch.rs`: accept `model` or `model_id` on `session.start` and `session.prompt`, validate request size, and forward it to the registry.
- `crates/lab/src/api/services/acp.rs`: accept `model`/`modelId` in create and prompt bodies, preserving the HTTP thin-shim pattern.
- `crates/lab/src/dispatch/acp/persistence.rs`: persist the new summary fields in SQLite using a user_version migration instead of relying on `CREATE TABLE IF NOT EXISTS`.
- `apps/gateway-admin/components/chat/types.ts`: add `ACPModelOption`, model fields on `ACPAgent`, and model fields on `ACPRun`.
- `apps/gateway-admin/lib/acp/types.ts`: mirror backend model/config fields in `ProviderHealth` and `BridgeSessionSummary`.
- `apps/gateway-admin/lib/chat/acp-normalizers.ts`: normalize provider/session model fields and extract model options from ACP config options.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`: add helpers for resolving valid model selection, creating model-matched sessions, and posting selected model with prompts.
- `apps/gateway-admin/lib/chat/chat-session-provider.tsx`: hold selected model state keyed by provider, reset invalid selections on adapter change, and expose `selectModel`.
- `apps/gateway-admin/components/chat/chat-input.tsx`: render a compact model selector scoped to the selected adapter.
- `apps/gateway-admin/components/chat/chat-shell.tsx`: pass model state/actions into `ChatInput`.
- `apps/gateway-admin/components/chat/chat-shell.test.tsx`: add focused unit tests for model validity and request payloads.
- `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`: add Playwright coverage for switching adapter/model without stale invalid model carryover.

Do not modify:

- `.gitignore`
- unrelated generated docs
- product code outside the files listed above unless a compiler error proves the boundary is incomplete

## Data Flow

```text
GET /v1/acp/provider
  -> provider list includes optional model options/default/current model
  -> frontend builds ACPAgent.models
  -> ChatInput renders adapter picker + model picker

User selects adapter
  -> selectedProviderId changes
  -> selected model resolves from provider-keyed selection, selected session, or adapter default
  -> invalid previous model is discarded

POST /v1/acp/sessions { provider, model? }
  -> dispatch session.start validates provider/model pair
  -> registry launches runtime and applies model config when supported
  -> AcpSessionSummary stores provider + model/config state

POST /v1/acp/sessions/{id}/prompt { prompt, model? }
  -> dispatch validates model against session/provider options
  -> registry applies config option before prompt if model differs
  -> runtime sends prompt
  -> session summary remains current for existing sessions
```

## Task 1: Backend DTOs And Normalizers

**Files:**
- Modify: `crates/lab-apis/src/acp/types.rs`
- Modify: `crates/lab/src/acp/types.rs`
- Modify: `apps/gateway-admin/components/chat/types.ts`
- Modify: `apps/gateway-admin/lib/acp/types.ts`
- Modify: `apps/gateway-admin/lib/chat/acp-normalizers.ts`
- Test: `apps/gateway-admin/components/chat/chat-shell.test.tsx`

- [ ] **Step 1: Write the failing frontend normalization test**

Add this test to `apps/gateway-admin/components/chat/chat-shell.test.tsx`:

```ts
test('normalizes adapter-scoped model options and selected run model', () => {
  const agentsWithModels: ACPAgent[] = [
    {
      id: 'codex-acp',
      name: 'Codex ACP',
      description: 'codex-acp over local ACP bridge',
      version: 'live',
      capabilities: [],
      models: [
        { id: 'gpt-5', name: 'GPT-5' },
        { id: 'gpt-5-mini', name: 'GPT-5 Mini' },
      ],
      defaultModelId: 'gpt-5',
      currentModelId: 'gpt-5-mini',
    },
  ]

  const selected = resolveSelectedAgent(agentsWithModels, 'codex-acp', {
    ...run('run-codex'),
    provider: 'codex-acp',
    modelId: 'gpt-5-mini',
    modelName: 'GPT-5 Mini',
  })

  assert.equal(selected.id, 'codex-acp')
  assert.equal(selected.models?.length, 2)
  assert.equal(selected.currentModelId, 'gpt-5-mini')
})
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cd apps/gateway-admin
pnpm test components/chat/chat-shell.test.tsx -- --test-name-pattern "normalizes adapter-scoped model options"
```

Expected: FAIL with TypeScript errors for missing `models`, `defaultModelId`, `currentModelId`, `modelId`, or `modelName`.

- [ ] **Step 3: Add shared frontend model types**

In `apps/gateway-admin/components/chat/types.ts`, extend the public chat types:

```ts
export interface ACPModelOption {
  id: string
  name: string
  description?: string | null
  fixed?: boolean
}

export interface ACPAgent {
  id: string
  name: string
  description: string
  version: string
  capabilities: string[]
  models?: ACPModelOption[]
  defaultModelId?: string | null
  currentModelId?: string | null
}

export interface ACPRun {
  id: string
  projectId: string
  agentId: string
  provider: string
  title: string
  createdAt: Date
  updatedAt: Date
  status: ACPRunStatus
  providerSessionId: string
  cwd: string
  modelId?: string | null
  modelName?: string | null
}
```

- [ ] **Step 4: Add frontend ACP wire fields**

In `apps/gateway-admin/lib/acp/types.ts`, extend `ProviderHealth` and `BridgeSessionSummary`:

```ts
export type ACPModelOption = {
  id: string
  name: string
  description?: string | null
  fixed?: boolean
}

export type ProviderHealth = {
  provider: AcpProviderKind
  ready: boolean
  command: string
  args: string[]
  message: string
  models?: ACPModelOption[]
  defaultModelId?: string | null
  currentModelId?: string | null
}

export type BridgeSessionSummary = {
  id: string
  providerSessionId: string
  provider: AcpProviderKind
  title: string
  cwd: string
  createdAt: string
  updatedAt: string
  status: BridgeSessionStatus
  agentName: string
  agentVersion: string
  resumable?: boolean
  modelId?: string | null
  modelName?: string | null
}
```

- [ ] **Step 5: Normalize raw session/provider model fields**

In `apps/gateway-admin/lib/chat/acp-normalizers.ts`, add `models`, `defaultModelId`, `currentModelId`, `modelId`, and `modelName` to the raw shapes. Use `model_id`/`model_name` fallbacks for Rust snake_case:

```ts
models?: Array<{ id: string; name: string; description?: string | null; fixed?: boolean }>
defaultModelId?: string | null
currentModelId?: string | null
default_model_id?: string | null
current_model_id?: string | null
modelId?: string | null
modelName?: string | null
model_id?: string | null
model_name?: string | null
```

Set `ACPRun.modelId` from `normalized.modelId ?? normalized.model_id ?? null` and `ACPRun.modelName` from `normalized.modelName ?? normalized.model_name ?? null`.

- [ ] **Step 6: Add backend API structs**

In `crates/lab-apis/src/acp/types.rs`, add these serializable structs and fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AcpModelOption {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fixed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AcpSessionConfigOptionView {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AcpModelOption>,
}
```

Add to `AcpSessionSummary`:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub model_id: Option<String>,
#[serde(skip_serializing_if = "Option::is_none")]
pub model_name: Option<String>,
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub config_options: Vec<AcpSessionConfigOptionView>,
```

Add to `AcpProviderHealth`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub models: Vec<AcpModelOption>,
#[serde(skip_serializing_if = "Option::is_none")]
pub default_model_id: Option<String>,
#[serde(skip_serializing_if = "Option::is_none")]
pub current_model_id: Option<String>,
```

- [ ] **Step 7: Run tests**

Run:

```bash
cd apps/gateway-admin
pnpm test components/chat/chat-shell.test.tsx -- --test-name-pattern "normalizes adapter-scoped model options"
cd ../..
cargo test --manifest-path crates/lab/Cargo.toml acp --all-features
```

Expected: frontend test PASS; Rust may fail in downstream constructors until Task 2 updates them.

## Task 2: Backend Session Model State And Persistence

**Files:**
- Modify: `crates/lab/src/acp/types.rs`
- Modify: `crates/lab/src/acp/runtime.rs`
- Modify: `crates/lab/src/acp/registry.rs`
- Modify: `crates/lab/src/dispatch/acp/persistence.rs`
- Test: `crates/lab/src/acp/registry.rs`
- Test: `crates/lab/src/dispatch/acp/persistence.rs`

- [ ] **Step 1: Write failing Rust tests**

Add registry tests that prove model state is preserved on create/list/get:

```rust
#[tokio::test]
async fn create_session_records_selected_model_when_valid_for_provider() {
    let registry = AcpSessionRegistry::new_for_test_with_provider_models(vec![(
        "codex".to_string(),
        vec![
            AcpModelOption { id: "gpt-5".into(), name: "GPT-5".into(), description: None, fixed: false },
            AcpModelOption { id: "gpt-5-mini".into(), name: "GPT-5 Mini".into(), description: None, fixed: false },
        ],
    )]);

    let summary = registry
        .create_session(
            StartSessionInput {
                provider: Some("codex".into()),
                cwd: ".".into(),
                title: Some("Model test".into()),
                principal: Some("user-1".into()),
                model_id: Some("gpt-5-mini".into()),
            },
            "user-1",
        )
        .await
        .expect("create session");

    assert_eq!(summary.model_id.as_deref(), Some("gpt-5-mini"));
    assert_eq!(summary.model_name.as_deref(), Some("GPT-5 Mini"));
}
```

Add a persistence test that inserts a summary with `model_id` and reloads it:

```rust
#[tokio::test]
async fn sqlite_persistence_round_trips_session_model_fields() {
    let db = test_persistence().await;
    let summary = AcpSessionSummary {
        id: "session-model".into(),
        provider: "codex".into(),
        title: "Model session".into(),
        cwd: "/tmp".into(),
        state: AcpSessionState::Idle,
        created_at: "2026-05-05T00:00:00Z".into(),
        updated_at: "2026-05-05T00:00:00Z".into(),
        principal: Some("user-1".into()),
        provider_session_id: Some("provider-session".into()),
        agent_name: Some("Codex".into()),
        agent_version: Some("1".into()),
        model_id: Some("gpt-5-mini".into()),
        model_name: Some("GPT-5 Mini".into()),
        config_options: Vec::new(),
    };

    db.save_session(&summary).await.expect("save");
    let loaded = db.list_sessions("user-1").await.expect("load").remove(0);

    assert_eq!(loaded.model_id.as_deref(), Some("gpt-5-mini"));
    assert_eq!(loaded.model_name.as_deref(), Some("GPT-5 Mini"));
}
```

- [ ] **Step 2: Run failing Rust tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml create_session_records_selected_model_when_valid_for_provider sqlite_persistence_round_trips_session_model_fields --all-features
```

Expected: FAIL because `StartSessionInput.model_id`, `AcpSessionSummary.model_id`, and persistence columns do not exist yet.

- [ ] **Step 3: Extend session input/result**

In `crates/lab/src/acp/types.rs`, extend `StartSessionInput`:

```rust
pub struct StartSessionInput {
    pub provider: Option<String>,
    pub cwd: String,
    pub title: Option<String>,
    pub principal: Option<String>,
    pub model_id: Option<String>,
}
```

Extend `StartSessionResult` if runtime can extract config options from `start_session()`:

```rust
pub struct StartSessionResult {
    pub provider_session_id: String,
    pub agent_name: String,
    pub agent_version: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub config_options: Vec<lab_apis::acp::types::AcpSessionConfigOptionView>,
}
```

- [ ] **Step 4: Add persistence migration**

In `crates/lab/src/dispatch/acp/persistence.rs`, add a user_version migration that adds:

```sql
ALTER TABLE acp_sessions ADD COLUMN model_id TEXT;
ALTER TABLE acp_sessions ADD COLUMN model_name TEXT;
ALTER TABLE acp_sessions ADD COLUMN config_options_json TEXT NOT NULL DEFAULT '[]';
```

Expected behavior:

- Existing databases migrate without dropping rows.
- New inserts write `model_id`, `model_name`, and serialized `config_options`.
- Reads tolerate invalid `config_options_json` by logging `decode_error` and returning an empty vector, not failing the whole session list.

- [ ] **Step 5: Validate model IDs in registry**

Add a small helper in `crates/lab/src/acp/registry.rs`:

```rust
fn resolve_model_selection(
    provider: &str,
    requested: Option<&str>,
    options: &[AcpModelOption],
    current: Option<&str>,
) -> Result<(Option<String>, Option<String>), ToolError> {
    let selected = requested.or(current);
    let Some(selected) = selected.filter(|value| !value.trim().is_empty()) else {
        return Ok((None, None));
    };
    let Some(option) = options.iter().find(|option| option.id == selected) else {
        return Err(ToolError::InvalidParam {
            message: format!("model `{selected}` is not valid for provider `{provider}`"),
            param: "model".to_string(),
        });
    };
    Ok((Some(option.id.clone()), Some(option.name.clone())))
}
```

- [ ] **Step 6: Run Rust tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml create_session_records_selected_model_when_valid_for_provider sqlite_persistence_round_trips_session_model_fields --all-features
```

Expected: PASS.

## Task 3: Apply ACP Model Config Before Prompt

**Files:**
- Modify: `crates/lab/src/acp/runtime.rs`
- Modify: `crates/lab/src/acp/registry.rs`
- Modify: `crates/lab/src/dispatch/acp/dispatch.rs`
- Test: `crates/lab/src/acp/runtime.rs`
- Test: `crates/lab/src/dispatch/acp/dispatch.rs`

- [ ] **Step 1: Write failing dispatch test**

Add a dispatch test proving invalid cross-adapter model selection is rejected:

```rust
#[tokio::test]
async fn session_prompt_rejects_model_not_valid_for_session_provider() {
    let registry = AcpSessionRegistry::new_for_test_with_provider_models(vec![(
        "codex".to_string(),
        vec![AcpModelOption {
            id: "gpt-5".into(),
            name: "GPT-5".into(),
            description: None,
            fixed: false,
        }],
    )]);
    let session = registry
        .create_session(
            StartSessionInput {
                provider: Some("codex".into()),
                cwd: ".".into(),
                title: None,
                principal: Some("user-1".into()),
                model_id: Some("gpt-5".into()),
            },
            "user-1",
        )
        .await
        .expect("create");

    let error = dispatch_with_registry(
        &registry,
        "session.prompt",
        serde_json::json!({
            "session_id": session.id,
            "principal": "user-1",
            "text": "hello",
            "model": "claude-sonnet-4.5"
        }),
    )
    .await
    .expect_err("invalid model should fail");

    assert_eq!(error.kind(), "invalid_param");
}
```

- [ ] **Step 2: Run failing test**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml session_prompt_rejects_model_not_valid_for_session_provider --all-features
```

Expected: FAIL because `model` is ignored or helper/test constructors are missing.

- [ ] **Step 3: Add model to prompt command**

Change `SessionCommand::Prompt(String)` in `crates/lab/src/acp/runtime.rs` to:

```rust
struct PromptCommand {
    prompt: String,
    model_id: Option<String>,
}

enum SessionCommand {
    Prompt(PromptCommand),
    Cancel,
}
```

Change `RuntimeHandle::prompt` to accept `model_id: Option<String>`.

- [ ] **Step 4: Apply config option before `send_prompt`**

Inside the `SessionCommand::Prompt` arm, before `session.send_prompt(prompt)`, call ACP `session/set_config_option` when:

- `model_id` is `Some`.
- The current config options contain a selector with category `model` or id `model`/`models`.
- The requested model differs from the current value.

Use `SetSessionConfigOptionRequest::new(session.session_id().to_string(), option_id, model_id)` and replace the stored config state with the response's full config options.

Expected error behavior:

- If the provider rejects the model, emit an `AcpEvent::ProviderInfo` or error event with kind/title that surfaces in chat activity, and return `invalid_param` to the HTTP caller.
- If the provider has no model config option and the selected model equals the fixed/default model, continue.
- If the provider has no model config option and the selected model differs, reject with `invalid_param`.

- [ ] **Step 5: Thread model through dispatch and registry**

In `crates/lab/src/dispatch/acp/dispatch.rs`, read both `model` and `model_id`:

```rust
let model_id = opt_str(&params, "model")
    .or_else(|| opt_str(&params, "model_id"))
    .map(str::to_string);
```

Pass it to `StartSessionInput` for `session.start` and to `registry.prompt_session(...)` for `session.prompt`.

- [ ] **Step 6: Run focused Rust tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml session_prompt_rejects_model_not_valid_for_session_provider --all-features
cargo test --manifest-path crates/lab/Cargo.toml acp::runtime --all-features
```

Expected: PASS.

## Task 4: HTTP Request Shape

**Files:**
- Modify: `crates/lab/src/api/services/acp.rs`
- Test: existing ACP API tests or add tests in `crates/lab/src/api/services/acp.rs`

- [ ] **Step 1: Write failing HTTP body test**

Add a test that posts:

```json
{
  "provider": "codex",
  "model": "gpt-5-mini"
}
```

to `POST /v1/acp/sessions`, and another that posts:

```json
{
  "prompt": "hello",
  "model": "gpt-5-mini"
}
```

to `POST /v1/acp/sessions/{session_id}/prompt`.

Assert the dispatch params contain `model: "gpt-5-mini"`.

- [ ] **Step 2: Run failing HTTP tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml acp --all-features
```

Expected: FAIL because `CreateSessionBody` and `PromptBody` do not deserialize model fields.

- [ ] **Step 3: Extend HTTP bodies**

In `crates/lab/src/api/services/acp.rs`:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionBody {
    provider: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    model: Option<String>,
    model_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptBody {
    prompt: String,
    page_context: Option<PageContextBody>,
    model: Option<String>,
    model_id: Option<String>,
}
```

When building dispatch params, set `"model"` to `body.model.or(body.model_id)`.

- [ ] **Step 4: Run focused HTTP tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml acp --all-features
```

Expected: PASS.

## Task 5: Frontend Model Selection State

**Files:**
- Modify: `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`
- Modify: `apps/gateway-admin/lib/chat/chat-session-provider.tsx`
- Test: `apps/gateway-admin/components/chat/chat-shell.test.tsx`

- [ ] **Step 1: Write failing controller tests**

Add tests:

```ts
test('resolveSelectedModel clears invalid model when adapter changes', () => {
  const codex: ACPAgent = {
    id: 'codex-acp',
    name: 'Codex ACP',
    description: '',
    version: 'live',
    capabilities: [],
    models: [{ id: 'gpt-5', name: 'GPT-5' }],
    defaultModelId: 'gpt-5',
  }
  const claude: ACPAgent = {
    id: 'claude-acp',
    name: 'Claude ACP',
    description: '',
    version: 'live',
    capabilities: [],
    models: [{ id: 'sonnet-4.5', name: 'Sonnet 4.5' }],
    defaultModelId: 'sonnet-4.5',
  }

  assert.equal(resolveSelectedModel(codex, 'sonnet-4.5', null)?.id, 'gpt-5')
  assert.equal(resolveSelectedModel(claude, 'gpt-5', null)?.id, 'sonnet-4.5')
})

test('sendPromptForSelectedProvider includes selected model', async () => {
  const requests: Array<{ body: unknown }> = []

  await sendPromptForSelectedProvider({
    payload: { text: 'hello', attachments: [] },
    selectedRun: { ...run('run-codex'), provider: 'codex-acp' },
    selectedProviderId: 'codex-acp',
    selectedModelId: 'gpt-5-mini',
    createSession: async () => run('unused'),
    isMobileViewport: false,
    fetchAcp: async (_path, init) => {
      requests.push({ body: JSON.parse(String(init?.body)) })
      return new Response(JSON.stringify({ ok: true }), { status: 200 })
    },
    refreshSessions: async () => {},
    addOptimisticMessage: () => {},
    removeOptimisticMessage: () => {},
  })

  assert.deepEqual(requests[0]?.body, { prompt: 'hello', model: 'gpt-5-mini' })
})
```

- [ ] **Step 2: Run failing frontend tests**

Run:

```bash
cd apps/gateway-admin
pnpm test components/chat/chat-shell.test.tsx -- --test-name-pattern "model"
```

Expected: FAIL because `resolveSelectedModel` and `selectedModelId` do not exist.

- [ ] **Step 3: Add model helper functions**

In `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`, add:

```ts
export function resolveSelectedModel(
  agent: ACPAgent | null,
  requestedModelId: string | null,
  selectedRun: ACPRun | null,
) {
  const models = agent?.models ?? []
  if (models.length === 0) return null
  const runModel = selectedRun?.provider === agent?.id ? selectedRun.modelId : null
  const candidate = requestedModelId ?? runModel ?? agent?.currentModelId ?? agent?.defaultModelId ?? null
  return models.find((model) => model.id === candidate) ?? models[0] ?? null
}
```

Extend `SendPromptForSelectedProviderOptions` with `selectedModelId?: string | null` and include `{ model: selectedModelId }` in the prompt body only when non-empty.

- [ ] **Step 4: Add provider-keyed selected model state**

In `apps/gateway-admin/lib/chat/chat-session-provider.tsx`:

- Add `const [selectedModelByProvider, setSelectedModelByProvider] = React.useState<Record<string, string>>({})`.
- Derive `selectedModel = resolveSelectedModel(selectedAgent, selectedModelByProvider[selectedAgent.id] ?? null, selectedRun)`.
- Add `selectModel(providerId: string, modelId: string)` that validates `modelId` against that provider's models before storing it.
- When selected provider changes, do not carry stale model IDs across providers; the resolver should choose that provider's current/default model.
- Expose `selectedModel` and `selectModel` through the existing chat contexts.

- [ ] **Step 5: Run frontend controller tests**

Run:

```bash
cd apps/gateway-admin
pnpm test components/chat/chat-shell.test.tsx -- --test-name-pattern "model"
```

Expected: PASS.

## Task 6: Chat Input Model Picker UI

**Files:**
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx`
- Test: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Write failing browser test**

Add a Playwright test after the existing adapter picker test:

```ts
test('chat shell model picker scopes models to the selected adapter', { concurrency: false }, async (t) => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  const sessions: BrowserSession[] = []
  const createRequests: Array<{ provider: string; model?: string }> = []
  const promptRequests: Array<{ sessionId: string; prompt: string; model?: string }> = []

  await mockAuthenticatedSession(page)
  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          providers: [
            {
              name: 'codex-acp',
              available: true,
              models: [{ id: 'gpt-5', name: 'GPT-5' }, { id: 'gpt-5-mini', name: 'GPT-5 Mini' }],
              defaultModelId: 'gpt-5',
            },
            {
              name: 'claude-acp',
              available: true,
              models: [{ id: 'sonnet-4.5', name: 'Sonnet 4.5' }],
              defaultModelId: 'sonnet-4.5',
            },
          ],
        }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ sessions }) })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'POST') {
      const payload = JSON.parse(request.postData() ?? '{}') as { provider?: string; model?: string }
      createRequests.push({ provider: payload.provider ?? 'codex-acp', model: payload.model })
      const created = session(`session-${sessions.length + 1}`, `${payload.provider} session`, payload.provider ?? 'codex-acp')
      sessions.unshift({ ...created, modelId: payload.model, modelName: payload.model })
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ session: sessions[0] }) })
      return
    }

    const promptMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/prompt$/)
    if (promptMatch && request.method() === 'POST') {
      const payload = JSON.parse(request.postData() ?? '{}') as { prompt?: string; model?: string }
      promptRequests.push({ sessionId: decodeURIComponent(promptMatch[1]!), prompt: payload.prompt ?? '', model: payload.model })
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ok: true }) })
      return
    }

    await route.fulfill({ status: 200, contentType: 'application/json', body: '{}' })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })
  await page.getByRole('button', { name: /Selected model: GPT-5/ }).click()
  await page.getByRole('option', { name: 'GPT-5 Mini' }).click()
  await page.getByRole('button', { name: 'Selected agent: Codex ACP' }).click()
  await page.getByRole('option', { name: /Claude ACP/ }).click()

  await assert.doesNotReject(() => page.getByRole('button', { name: /Selected model: Sonnet 4.5/ }).waitFor())
  await page.getByRole('textbox', { name: 'Message' }).fill('Use scoped model')
  await page.getByRole('button', { name: 'Send message' }).click()

  await waitForCondition(() => promptRequests.length === 1)
  assert.deepEqual(createRequests, [{ provider: 'claude-acp', model: 'sonnet-4.5' }])
  assert.deepEqual(promptRequests, [{ sessionId: 'session-1', prompt: 'Use scoped model', model: 'sonnet-4.5' }])
})
```

- [ ] **Step 2: Run failing browser test**

Run:

```bash
cd apps/gateway-admin
pnpm test lib/browser/chat-shell.browser.test.ts -- --test-name-pattern "model picker scopes models"
```

Expected: FAIL because the model picker is not rendered.

- [ ] **Step 3: Render model picker**

In `apps/gateway-admin/components/chat/chat-input.tsx`:

- Add props:

```ts
selectedModel: ACPModelOption | null
modelOptions: ACPModelOption[]
onSelectModel: (modelId: string) => void
```

- Render a compact selector next to the adapter selector only when `modelOptions.length > 0`.
- Use `aria-label={selectedModel ? \`Selected model: ${selectedModel.name}\` : 'Select model'}`.
- Disable the selector when only one fixed/default model exists, but still display the current model name.
- Keyboard behavior should mirror the existing adapter listbox: Escape closes, ArrowUp/ArrowDown moves, Enter/Space selects.

- [ ] **Step 4: Pass model props from shell**

In `apps/gateway-admin/components/chat/chat-shell.tsx`, read `selectedModel` from `useChatSessionData()` and `selectModel` from `useChatSessionActions()`. Pass:

```tsx
selectedModel={selectedModel}
modelOptions={selectedAgent.models ?? []}
onSelectModel={(modelId) => selectModel(selectedAgent.id, modelId)}
```

- [ ] **Step 5: Run browser test**

Run:

```bash
cd apps/gateway-admin
pnpm test lib/browser/chat-shell.browser.test.ts -- --test-name-pattern "model picker scopes models"
```

Expected: PASS. The prompt request carries only `sonnet-4.5`, not stale `gpt-5-mini`.

## Task 7: Existing Session Display

**Files:**
- Modify: `apps/gateway-admin/components/chat/session-sidebar.tsx`
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx`
- Test: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Write failing browser assertion**

Extend the model picker browser test so the mocked `GET /v1/acp/sessions` returns an existing session:

```ts
sessions.unshift({
  ...session('existing-codex', 'Existing Codex', 'codex-acp'),
  modelId: 'gpt-5-mini',
  modelName: 'GPT-5 Mini',
})
```

Assert:

```ts
await assert.doesNotReject(() =>
  page.getByText('GPT-5 Mini').waitFor(),
)
```

- [ ] **Step 2: Run failing test**

Run:

```bash
cd apps/gateway-admin
pnpm test lib/browser/chat-shell.browser.test.ts -- --test-name-pattern "model picker scopes models"
```

Expected: FAIL because existing sessions do not show model names.

- [ ] **Step 3: Display model in current session context**

In `apps/gateway-admin/components/chat/chat-shell.tsx`, include the selected run model in the header after the run title:

```tsx
{selectedRun.modelName && (
  <>
    <span className="hidden text-aurora-text-muted/30 sm:block">/</span>
    <span className="max-w-[120px] truncate text-aurora-text-muted sm:max-w-[180px]">
      {selectedRun.modelName}
    </span>
  </>
)}
```

In `apps/gateway-admin/components/chat/session-sidebar.tsx`, render a small secondary model line or suffix under each session title when `run.modelName` is present.

- [ ] **Step 4: Run browser test**

Run:

```bash
cd apps/gateway-admin
pnpm test lib/browser/chat-shell.browser.test.ts -- --test-name-pattern "model picker scopes models"
```

Expected: PASS.

## Task 8: Verification And Full Gates

**Files:**
- No new source files unless earlier tasks require scoped helper modules.

- [ ] **Step 1: Run focused frontend tests**

Run:

```bash
cd apps/gateway-admin
pnpm test components/chat/chat-shell.test.tsx
pnpm test lib/browser/chat-shell.browser.test.ts -- --test-name-pattern "model|agent picker"
```

Expected: PASS. Existing adapter switching tests still pass and new model tests pass.

- [ ] **Step 2: Run frontend build/type gate**

Run:

```bash
cd apps/gateway-admin
pnpm build
```

Expected: PASS with static export. No model selector text overflow at desktop/mobile widths.

- [ ] **Step 3: Run focused Rust tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml acp --all-features
```

Expected: PASS. ACP session create, prompt, persistence, and API tests pass.

- [ ] **Step 4: Run default repo gate**

Run:

```bash
just test
```

Expected: `cargo nextest run --workspace --all-features` passes. If unrelated failures occur, capture the exact failing test names and rerun the focused commands above to prove the model-switching slice.

- [ ] **Step 5: Manual smoke**

Run:

```bash
just chat-local
```

Open the chat route. Expected behavior:

- Adapter picker shows available adapters.
- Model picker shows only models for the selected adapter.
- Switching from `codex-acp` to `claude-acp` replaces any invalid Codex model with Claude's default/current model.
- Sending a prompt creates or reuses a session for the selected adapter and sends only that adapter's valid model.
- Existing sessions show their current model when present.

## Not In Scope

- User-level saved model preferences across browser reloads.
- Marketplace/provider install UI for editing model catalogs.
- Global model routing outside chat.
- Direct use of ACP `session/set_model` except as a future compatibility fallback; prefer `session/set_config_option` category `model`.
- Polling provider model lists in the background.

## Completion Checklist

- [ ] `bd show lab-wozy --json` includes the plan path or planning comments.
- [ ] `docs/superpowers/plans/2026-05-05-chat-adapter-model-switching.md` exists.
- [ ] `.gitignore` remains untouched by this work.
- [ ] Product code changes are left for a future implementation pass.
