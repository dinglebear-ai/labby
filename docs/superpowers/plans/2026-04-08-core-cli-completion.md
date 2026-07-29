# Core CLI Completion Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire all 21 services into the catalog, registry, MCP dispatch, HTTP API, and doctor/health — so adding a new service means only implementing it, never touching core files.

**Architecture:** Each MCP dispatcher exports `pub const ACTIONS: &[lab_apis::core::ActionSpec]` and an `async fn dispatch()`. `catalog.rs` converts those slices via a shared `convert_actions()` helper. `registry.rs`, `serve.rs`, and the HTTP router all iterate the same feature-gated service list. `doctor` and `health` drive from `PluginMeta` constants, not hardcoded service blocks.

**Tech Stack:** Rust 2024 edition, cargo-nextest, axum 0.8, clap derive, anyhow

---

## File Map

**Modified:**
- `crates/lab/src/mcp/registry.rs` — remove retired-upstream cfg gate from `category_slug`, register all 21 services
- `crates/lab/src/mcp/services.rs` — declare all 21 dispatcher modules
- `crates/lab/src/mcp/services/extract.rs` — migrate local ActionSpec stand-ins to real types
- `crates/lab/src/mcp/services/retired-upstream.rs` — add `pub const ACTIONS`
- `crates/lab/src/cli/serve.rs` — add all 21 services to dispatch match
- `crates/lab/src/catalog.rs` — add `convert_actions()`, wire all 21 services
- `crates/lab/src/cli.rs` — add retired-upstream subcommand + service stubs
- `crates/lab/src/cli/doctor.rs` — generic over all services via META list
- `crates/lab/src/cli/health.rs` — generic over all services via service list
- `crates/lab/src/api/state.rs` — add clients for all real-SDK services
- `crates/lab/src/api/router.rs` — overhaul to `POST /v1/<service>` action dispatch
- `crates/lab/src/api.rs` — declare `services` module

**Created:**
- `crates/lab/src/mcp/services/{retired-upstream,retired-upstream,retired-upstream,retired-upstream,retired-upstream,retired-upstream,tailscale,linkding,memos,bytestash,paperless,arcane,unraid,unifi,retired-upstream,gotify,openai,qdrant,tei,apprise}.rs` — 20 dispatcher stubs (arcane, retired-upstream, retired-upstream, openai, retired-upstream already exist as empty files; 15 need to be created)
- `crates/lab/src/cli/retired-upstream.rs` — reference CLI shim
- `crates/lab/src/cli/{retired-upstream,retired-upstream,retired-upstream,retired-upstream,retired-upstream,retired-upstream,tailscale,linkding,memos,bytestash,paperless,arcane,unraid,unifi,retired-upstream,gotify,openai,qdrant,tei,apprise}.rs` — CLI stubs (some exist, some need creating)
- `crates/lab/src/api/services.rs` — module declaration
- `crates/lab/src/api/services/{extract,retired-upstream,retired-upstream,...}.rs` — per-service HTTP handlers

---

## Task 1: Make `category_slug` always available

**Files:**
- Modify: `crates/lab/src/mcp/registry.rs`

The function is currently gated `#[cfg(feature = "retired-upstream")]` but it only uses `lab_apis::core::Category`, which is always available. This blocks registering any other service.

- [ ] **Step 1: Remove the cfg gate from `category_slug`**

In `crates/lab/src/mcp/registry.rs`, replace:

```rust
#[cfg(feature = "retired-upstream")]
const fn category_slug(cat: lab_apis::core::Category) -> &'static str {
    use lab_apis::core::Category;
    match cat {
        Category::Media => "media",
        Category::Retired upstream => "retired-upstream",
        Category::Indexer => "indexer",
        Category::Download => "download",
        Category::Notes => "notes",
        Category::Documents => "documents",
        Category::Network => "network",
        Category::Notifications => "notifications",
        Category::Ai => "ai",
        Category::Bootstrap => "bootstrap",
    }
}
```

with:

```rust
const fn category_slug(cat: lab_apis::core::Category) -> &'static str {
    use lab_apis::core::Category;
    match cat {
        Category::Media => "media",
        Category::Retired upstream => "retired-upstream",
        Category::Indexer => "indexer",
        Category::Download => "download",
        Category::Notes => "notes",
        Category::Documents => "documents",
        Category::Network => "network",
        Category::Notifications => "notifications",
        Category::Ai => "ai",
        Category::Bootstrap => "bootstrap",
    }
}
```

- [ ] **Step 2: Verify it still compiles**

```bash
rtk cargo check -p lab --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
rtk git add crates/lab/src/mcp/registry.rs
rtk git commit -m "fix(mcp): make category_slug always available, not retired-upstream-gated"
```

---

## Task 2: Migrate extract dispatcher to real ActionSpec types

**Files:**
- Modify: `crates/lab/src/mcp/services/extract.rs`

The extract dispatcher defines its own local `ActionSpec` / `ParamSpec` structs with a TODO comment to switch to the real types once they existed. They exist now in `lab_apis::core::action`. The difference: the real `ActionSpec` has a `returns: &'static str` field.

- [ ] **Step 1: Write the failing test**

In `crates/lab/src/mcp/services/extract.rs` add at the bottom (inside a `#[cfg(test)]` block):

```rust
#[cfg(test)]
mod tests {
    use super::ACTIONS;
    use lab_apis::core::ActionSpec;

    #[test]
    fn actions_use_real_types() {
        // If ACTIONS is &[ActionSpec] from lab_apis::core, this compiles.
        let _: &[ActionSpec] = ACTIONS;
        assert!(!ACTIONS.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
rtk cargo test -p lab --all-features -- mcp::services::extract::tests 2>&1 | tail -20
```

Expected: compile error — `ACTIONS` is `&[local::ActionSpec]`, not `&[lab_apis::core::ActionSpec]`.

- [ ] **Step 3: Replace local stand-ins with real types**

In `crates/lab/src/mcp/services/extract.rs`:

1. Remove the local `ActionSpec` and `ParamSpec` struct definitions at the bottom of the file (the section starting with `// ─── Local stand-ins`).

2. Add this import at the top with the other `use` statements:

```rust
use lab_apis::core::action::{ActionSpec, ParamSpec};
```

3. Add `returns: ""` to each `ActionSpec` literal in `ACTIONS` (the real type has a `returns` field):

```rust
pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "scan",
        description: "Scan an appdata path and return discovered service credentials",
        destructive: false,
        returns: "DiscoveredService[]",
        params: &[
            ParamSpec { name: "uri", ty: "string", required: true,
                description: "Local path or 'host:/abs/path' for SSH" },
        ],
    },
    ActionSpec {
        name: "apply",
        description: "Scan and write discovered credentials into ~/.labby/.env (with backup)",
        destructive: true,
        returns: "WritePlan",
        params: &[
            ParamSpec { name: "uri", ty: "string", required: true,
                description: "Same as scan" },
            ParamSpec { name: "services", ty: "string[]", required: false,
                description: "Optional filter; defaults to everything found" },
            ParamSpec { name: "env_path", ty: "string", required: false,
                description: "Override target env file path" },
        ],
    },
    ActionSpec {
        name: "diff",
        description: "Show what 'apply' would change vs the current env file (no writes)",
        destructive: false,
        returns: "WritePlan",
        params: &[
            ParamSpec { name: "uri", ty: "string", required: true, description: "" },
        ],
    },
    ActionSpec {
        name: "help",
        description: "Show this catalog, or one action's detail with params.action='<name>'",
        destructive: false,
        returns: "Catalog",
        params: &[],
    },
];
```

- [ ] **Step 4: Run test to verify it passes**

```bash
rtk cargo test -p lab --all-features -- mcp::services::extract::tests 2>&1 | tail -10
```

Expected: `test mcp::services::extract::tests::actions_use_real_types ... ok`

- [ ] **Step 5: Commit**

```bash
rtk git add crates/lab/src/mcp/services/extract.rs
rtk git commit -m "fix(extract): migrate local ActionSpec stand-ins to real lab_apis::core types"
```

---

## Task 3: Add `pub const ACTIONS` to Retired upstream and fix catalog builder

**Files:**
- Modify: `crates/lab/src/mcp/services/retired-upstream.rs`
- Modify: `crates/lab/src/catalog.rs`

The retired-upstream dispatcher duplicates its action list in two places: the hardcoded `Vec` in `catalog.rs::actions_for()` and the help arm of `dispatch()`. Consolidate to a single `pub const ACTIONS`.

- [ ] **Step 1: Write the failing test**

In `crates/lab/src/mcp/services/retired-upstream.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::ACTIONS;

    #[test]
    fn retired-upstream_has_system_status_action() {
        assert!(ACTIONS.iter().any(|a| a.name == "system.status"));
    }

    #[test]
    fn retired-upstream_has_help_action() {
        assert!(ACTIONS.iter().any(|a| a.name == "help"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
rtk cargo test -p lab --features retired-upstream -- mcp::services::retired-upstream::tests 2>&1 | tail -10
```

Expected: compile error — `ACTIONS` not defined.

- [ ] **Step 3: Add `pub const ACTIONS` to retired-upstream dispatcher**

In `crates/lab/src/mcp/services/retired-upstream.rs`, add these imports and constant after the existing `use` lines:

```rust
use lab_apis::core::action::{ActionSpec, ParamSpec};

/// Action catalog for the retired-upstream tool.
/// Read by `catalog.rs`, `mcp/resources.rs`, and the `help` dispatch arm.
pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "system.status",
        description: "Return Retired upstream system status and version",
        destructive: false,
        returns: "SystemStatus",
        params: &[],
    },
    ActionSpec {
        name: "help",
        description: "Show this catalog",
        destructive: false,
        returns: "Catalog",
        params: &[],
    },
];
```

Then replace the hardcoded help arm in `dispatch()`:

```rust
"help" => Ok(serde_json::json!({
    "service": "retired-upstream",
    "actions": ACTIONS.iter().map(|a| serde_json::json!({
        "name": a.name,
        "description": a.description,
        "destructive": a.destructive,
        "params": a.params.iter().map(|p| serde_json::json!({
            "name": p.name,
            "type": p.ty,
            "required": p.required,
            "description": p.description,
        })).collect::<Vec<_>>(),
    })).collect::<Vec<_>>(),
})),
```

- [ ] **Step 4: Add `convert_actions` helper to catalog and wire extract + retired-upstream**

Replace the entire contents of `crates/lab/src/catalog.rs` with:

```rust
//! Shared catalog module — single source of truth for service + action
//! discovery, feeding three surfaces: the `lab.help` MCP meta-tool, the
//! `lab://catalog` MCP resource, and the `lab help` CLI subcommand.

use serde::{Deserialize, Serialize};

use crate::mcp::registry::ToolRegistry;

/// Top-level discovery document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    /// One entry per registered service.
    pub services: Vec<ServiceCatalog>,
}

/// Per-service slice of the discovery document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCatalog {
    /// Service identifier (matches the MCP tool name and CLI subcommand).
    pub name: String,
    /// Short human description from `PluginMeta::description`.
    pub description: String,
    /// Category slug (Media, Retired upstream, Notifications, etc.).
    pub category: String,
    /// List of actions exposed by the service.
    pub actions: Vec<ActionEntry>,
}

/// One action inside a service's catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEntry {
    /// Dotted action name (e.g., `movie.search`).
    pub name: String,
    /// Short description.
    pub description: String,
    /// Whether the action mutates state and requires confirmation.
    pub destructive: bool,
}

/// Build a [`Catalog`] from the current tool registry. The registry is
/// the authoritative source — never hand-roll catalog entries.
#[must_use]
pub fn build_catalog(registry: &ToolRegistry) -> Catalog {
    let services = registry
        .services()
        .iter()
        .map(|svc| ServiceCatalog {
            name: svc.name.to_string(),
            description: svc.description.to_string(),
            category: svc.category.to_string(),
            actions: actions_for(svc.name),
        })
        .collect();

    Catalog { services }
}

/// Convert a service's `&[ActionSpec]` into `Vec<ActionEntry>` for the catalog.
fn convert_actions(specs: &[lab_apis::core::ActionSpec]) -> Vec<ActionEntry> {
    specs
        .iter()
        .map(|s| ActionEntry {
            name: s.name.into(),
            description: s.description.into(),
            destructive: s.destructive,
        })
        .collect()
}

fn actions_for(service: &str) -> Vec<ActionEntry> {
    match service {
        "extract" => convert_actions(crate::mcp::services::extract::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        _ => Vec::new(),
    }
}
```

- [ ] **Step 5: Run tests to verify**

```bash
rtk cargo test -p lab --features retired-upstream -- mcp::services::retired-upstream::tests 2>&1 | tail -10
```

Expected: both retired-upstream tests pass.

- [ ] **Step 6: Compile check**

```bash
rtk cargo check -p lab --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/lab/src/mcp/services/retired-upstream.rs crates/lab/src/catalog.rs
rtk git commit -m "feat(catalog): add ACTIONS to retired-upstream dispatcher, drive catalog from ACTIONS slices"
```

---

## Task 4: Register extract in registry and dispatch

**Files:**
- Modify: `crates/lab/src/mcp/registry.rs`
- Modify: `crates/lab/src/cli/serve.rs`

Extract is always-on but currently not registered in the registry or dispatch match — so `lab help` doesn't list it and `lab serve` can't dispatch to it.

- [ ] **Step 1: Write the failing test**

In `crates/lab/src/mcp/registry.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::build_default_registry;

    #[test]
    fn extract_is_always_registered() {
        let reg = build_default_registry();
        assert!(
            reg.services().iter().any(|s| s.name == "extract"),
            "extract must be in the default registry"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
rtk cargo test -p lab --all-features -- mcp::registry::tests 2>&1 | tail -10
```

Expected: `FAILED` — extract not found in registry.

- [ ] **Step 3: Register extract in `build_default_registry`**

In `crates/lab/src/mcp/registry.rs`, add the extract block before the retired-upstream block in `build_default_registry`:

```rust
#[must_use]
pub fn build_default_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    // extract is always-on (no feature flag).
    {
        let meta = lab_apis::extract::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    reg
}
```

- [ ] **Step 4: Add extract arm to `serve.rs` dispatch match**

In `crates/lab/src/cli/serve.rs`, update the `dispatch` function:

```rust
async fn dispatch(
    registry: &ToolRegistry,
    service: &str,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    if !registry.services().iter().any(|s| s.name == service) {
        anyhow::bail!("unknown service `{service}`");
    }
    match service {
        "extract" => crate::mcp::services::extract::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        other => anyhow::bail!("service `{other}` has no dispatcher wired"),
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

```bash
rtk cargo test -p lab --all-features -- mcp::registry::tests 2>&1 | tail -10
```

Expected: `test mcp::registry::tests::extract_is_always_registered ... ok`

- [ ] **Step 6: Commit**

```bash
rtk git add crates/lab/src/mcp/registry.rs crates/lab/src/cli/serve.rs
rtk git commit -m "feat(mcp): register extract in default registry and serve dispatch"
```

---

## Task 5: Create dispatcher stubs for all 20 feature-gated services

**Files:**
- Modify: `crates/lab/src/mcp/services.rs`
- Modify: `crates/lab/src/mcp/registry.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/catalog.rs`
- Create: 20 stub dispatcher files

Each feature-gated service needs a dispatcher module with `pub const ACTIONS: &[ActionSpec]` (empty until implemented) and a `dispatch()` that returns a "not yet implemented" error. This unlocks registering them so `lab help` lists them all even before their client logic is written.

The 20 services are: `retired-upstream` (already done), `retired-upstream`, `retired-upstream`, `retired-upstream`, `retired-upstream`, `retired-upstream`, `retired-upstream`, `tailscale`, `linkding`, `memos`, `bytestash`, `paperless`, `arcane`, `unraid`, `unifi`, `retired-upstream`, `gotify`, `openai`, `qdrant`, `tei`, `apprise`.

Stubs for `retired-upstream`, `retired-upstream`, `retired-upstream`, `arcane`, `openai` already exist as 25-byte placeholder files — they need to be replaced with real stubs. The other 14 need to be created.

- [ ] **Step 1: Generate all 19 stub dispatcher files**

Run this script from the workspace root:

```bash
cd /home/jmagar/workspace/lab

SERVICES=(retired-upstream retired-upstream retired-upstream retired-upstream retired-upstream retired-upstream tailscale linkding memos bytestash paperless arcane unraid unifi retired-upstream gotify openai qdrant tei apprise)

for svc in "${SERVICES[@]}"; do
cat > "crates/lab/src/mcp/services/${svc}.rs" << RUST
//! MCP dispatch stub for the \`${svc}\` tool.
//!
//! Replace this stub with a real implementation when the service client
//! is ready. See \`retired-upstream.rs\` for the reference pattern.

use anyhow::Result;
use serde_json::Value;

use lab_apis::core::action::{ActionSpec, ParamSpec};

/// Action catalog — empty until service is implemented.
pub const ACTIONS: &[ActionSpec] = &[];

/// Dispatch one MCP call against the ${svc} tool.
///
/// # Errors
/// Returns \`not_implemented\` for all actions until the service is wired.
pub async fn dispatch(action: &str, _params: Value) -> Result<Value> {
    match action {
        "help" => Ok(serde_json::json!({
            "service": "${svc}",
            "message": "${svc} is not yet implemented",
            "actions": []
        })),
        _ => anyhow::bail!("${svc} is not yet implemented — action: {action}"),
    }
}
RUST
done
echo "Created ${#SERVICES[@]} dispatcher stubs"
```

- [ ] **Step 2: Declare all 20 modules in `services.rs`**

Replace `crates/lab/src/mcp/services.rs` with:

```rust
//! Per-service dispatch modules.
//!
//! Every module exports `pub const ACTIONS: &[ActionSpec]` and
//! `pub async fn dispatch(action: &str, params: Value) -> Result<Value>`.
//! See `retired-upstream.rs` for the reference implementation.

pub mod extract;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "tailscale")]
pub mod tailscale;

#[cfg(feature = "linkding")]
pub mod linkding;

#[cfg(feature = "memos")]
pub mod memos;

#[cfg(feature = "bytestash")]
pub mod bytestash;

#[cfg(feature = "paperless")]
pub mod paperless;

#[cfg(feature = "arcane")]
pub mod arcane;

#[cfg(feature = "unraid")]
pub mod unraid;

#[cfg(feature = "unifi")]
pub mod unifi;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "gotify")]
pub mod gotify;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "qdrant")]
pub mod qdrant;

#[cfg(feature = "tei")]
pub mod tei;

#[cfg(feature = "apprise")]
pub mod apprise;
```

- [ ] **Step 3: Register all 20 services in `build_default_registry`**

Replace the body of `build_default_registry()` in `registry.rs` with:

```rust
#[must_use]
pub fn build_default_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    // extract is always-on (no feature flag).
    {
        let meta = lab_apis::extract::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "tailscale")]
    {
        let meta = lab_apis::tailscale::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "linkding")]
    {
        let meta = lab_apis::linkding::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "memos")]
    {
        let meta = lab_apis::memos::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "bytestash")]
    {
        let meta = lab_apis::bytestash::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "paperless")]
    {
        let meta = lab_apis::paperless::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "arcane")]
    {
        let meta = lab_apis::arcane::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "unraid")]
    {
        let meta = lab_apis::unraid::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "unifi")]
    {
        let meta = lab_apis::unifi::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "gotify")]
    {
        let meta = lab_apis::gotify::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "openai")]
    {
        let meta = lab_apis::openai::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "qdrant")]
    {
        let meta = lab_apis::qdrant::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "tei")]
    {
        let meta = lab_apis::tei::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    #[cfg(feature = "apprise")]
    {
        let meta = lab_apis::apprise::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    reg
}
```

- [ ] **Step 4: Add all 20 arms to `serve.rs` dispatch match**

Replace the `dispatch` function in `crates/lab/src/cli/serve.rs`:

```rust
async fn dispatch(
    registry: &ToolRegistry,
    service: &str,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    if !registry.services().iter().any(|s| s.name == service) {
        anyhow::bail!("unknown service `{service}`");
    }
    match service {
        "extract" => crate::mcp::services::extract::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "tailscale")]
        "tailscale" => crate::mcp::services::tailscale::dispatch(action, params).await,
        #[cfg(feature = "linkding")]
        "linkding" => crate::mcp::services::linkding::dispatch(action, params).await,
        #[cfg(feature = "memos")]
        "memos" => crate::mcp::services::memos::dispatch(action, params).await,
        #[cfg(feature = "bytestash")]
        "bytestash" => crate::mcp::services::bytestash::dispatch(action, params).await,
        #[cfg(feature = "paperless")]
        "paperless" => crate::mcp::services::paperless::dispatch(action, params).await,
        #[cfg(feature = "arcane")]
        "arcane" => crate::mcp::services::arcane::dispatch(action, params).await,
        #[cfg(feature = "unraid")]
        "unraid" => crate::mcp::services::unraid::dispatch(action, params).await,
        #[cfg(feature = "unifi")]
        "unifi" => crate::mcp::services::unifi::dispatch(action, params).await,
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        #[cfg(feature = "gotify")]
        "gotify" => crate::mcp::services::gotify::dispatch(action, params).await,
        #[cfg(feature = "openai")]
        "openai" => crate::mcp::services::openai::dispatch(action, params).await,
        #[cfg(feature = "qdrant")]
        "qdrant" => crate::mcp::services::qdrant::dispatch(action, params).await,
        #[cfg(feature = "tei")]
        "tei" => crate::mcp::services::tei::dispatch(action, params).await,
        #[cfg(feature = "apprise")]
        "apprise" => crate::mcp::services::apprise::dispatch(action, params).await,
        other => anyhow::bail!("service `{other}` has no dispatcher wired"),
    }
}
```

- [ ] **Step 5: Wire all 20 services in `catalog.rs::actions_for`**

Replace `actions_for()` in `crates/lab/src/catalog.rs`:

```rust
fn actions_for(service: &str) -> Vec<ActionEntry> {
    match service {
        "extract" => convert_actions(crate::mcp::services::extract::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "tailscale")]
        "tailscale" => convert_actions(crate::mcp::services::tailscale::ACTIONS),
        #[cfg(feature = "linkding")]
        "linkding" => convert_actions(crate::mcp::services::linkding::ACTIONS),
        #[cfg(feature = "memos")]
        "memos" => convert_actions(crate::mcp::services::memos::ACTIONS),
        #[cfg(feature = "bytestash")]
        "bytestash" => convert_actions(crate::mcp::services::bytestash::ACTIONS),
        #[cfg(feature = "paperless")]
        "paperless" => convert_actions(crate::mcp::services::paperless::ACTIONS),
        #[cfg(feature = "arcane")]
        "arcane" => convert_actions(crate::mcp::services::arcane::ACTIONS),
        #[cfg(feature = "unraid")]
        "unraid" => convert_actions(crate::mcp::services::unraid::ACTIONS),
        #[cfg(feature = "unifi")]
        "unifi" => convert_actions(crate::mcp::services::unifi::ACTIONS),
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => convert_actions(crate::mcp::services::retired-upstream::ACTIONS),
        #[cfg(feature = "gotify")]
        "gotify" => convert_actions(crate::mcp::services::gotify::ACTIONS),
        #[cfg(feature = "openai")]
        "openai" => convert_actions(crate::mcp::services::openai::ACTIONS),
        #[cfg(feature = "qdrant")]
        "qdrant" => convert_actions(crate::mcp::services::qdrant::ACTIONS),
        #[cfg(feature = "tei")]
        "tei" => convert_actions(crate::mcp::services::tei::ACTIONS),
        #[cfg(feature = "apprise")]
        "apprise" => convert_actions(crate::mcp::services::apprise::ACTIONS),
        _ => Vec::new(),
    }
}
```

- [ ] **Step 6: Write a registry test asserting all services appear**

In `crates/lab/src/mcp/registry.rs`, add to the `tests` module:

```rust
#[test]
fn all_features_registers_all_services() {
    let reg = build_default_registry();
    let names: Vec<&str> = reg.services().iter().map(|s| s.name).collect();
    // extract is always present
    assert!(names.contains(&"extract"), "extract missing from registry");
    // spot-check a few feature-gated ones (compiled in with --all-features)
    #[cfg(feature = "retired-upstream")]
    assert!(names.contains(&"retired-upstream"), "retired-upstream missing from registry");
    #[cfg(feature = "retired-upstream")]
    assert!(names.contains(&"retired-upstream"), "retired-upstream missing from registry");
    #[cfg(feature = "apprise")]
    assert!(names.contains(&"apprise"), "apprise missing from registry");
}
```

- [ ] **Step 7: Run tests**

```bash
rtk cargo test -p lab --all-features -- mcp::registry::tests 2>&1 | tail -15
```

Expected: both registry tests pass.

- [ ] **Step 8: Compile check**

```bash
rtk cargo check -p lab --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 9: Commit**

```bash
rtk git add \
  crates/lab/src/mcp/services.rs \
  crates/lab/src/mcp/services/*.rs \
  crates/lab/src/mcp/registry.rs \
  crates/lab/src/cli/serve.rs \
  crates/lab/src/catalog.rs
rtk git commit -m "feat(mcp): wire all 21 services into registry, dispatch, and catalog"
```

---

## Task 6: Add the Retired upstream CLI shim (reference implementation)

**Files:**
- Create: `crates/lab/src/cli/retired-upstream.rs`
- Modify: `crates/lab/src/cli.rs`

This is the template every other CLI shim will follow: ~20 lines, parse args, call client, format output. No business logic.

- [ ] **Step 1: Write the failing test**

Create `crates/lab/src/cli/retired-upstream.rs` with just the test:

```rust
//! `lab retired-upstream` — CLI shim for the Retired upstream service.

#[cfg(test)]
mod tests {
    #[test]
    fn system_status_args_parse() {
        use clap::Parser;
        use super::Retired upstreamArgs;

        // Subcommand `lab retired-upstream system-status` should parse.
        let args = Retired upstreamArgs::try_parse_from(["retired-upstream", "system-status"]);
        assert!(args.is_ok(), "failed to parse: {args:?}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
rtk cargo test -p lab --features retired-upstream -- cli::retired-upstream::tests 2>&1 | tail -10
```

Expected: compile error — `Retired upstreamArgs` not defined.

- [ ] **Step 3: Implement the Retired upstream CLI shim**

Replace the contents of `crates/lab/src/cli/retired-upstream.rs` with:

```rust
//! `lab retired-upstream` — CLI shim for the Retired upstream service.
//!
//! Thin shim: parse → call client → format. No business logic here.
//! See `crates/lab/src/cli/CLAUDE.md` for the shim rules.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::output::{OutputFormat, print};

/// `lab retired-upstream` arguments.
#[derive(Debug, Args)]
pub struct Retired upstreamArgs {
    #[command(subcommand)]
    pub command: Retired upstreamCommand,
}

/// Retired upstream subcommands.
#[derive(Debug, Subcommand)]
pub enum Retired upstreamCommand {
    /// Return Retired upstream system status and version.
    SystemStatus,
}

/// Run the `lab retired-upstream` subcommand.
///
/// # Errors
/// Returns an error if the client is not configured or the API call fails.
pub async fn run(args: Retired upstreamArgs, format: OutputFormat) -> Result<ExitCode> {
    let client = crate::mcp::services::retired-upstream::client_from_env()
        .ok_or_else(|| anyhow::anyhow!("RETIRED_UPSTREAM_URL and RETIRED_UPSTREAM_API_KEY must be set"))?;

    match args.command {
        Retired upstreamCommand::SystemStatus => {
            let status = client.system_status().await?;
            print(&status, format)?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_status_args_parse() {
        use clap::Parser;
        use super::Retired upstreamArgs;

        let args = Retired upstreamArgs::try_parse_from(["retired-upstream", "system-status"]);
        assert!(args.is_ok(), "failed to parse: {args:?}");
    }
}
```

- [ ] **Step 4: Wire the retired-upstream subcommand in `cli.rs`**

In `crates/lab/src/cli.rs`, add `retired-upstream` to the module list and enum:

```rust
pub mod completions;
pub mod doctor;
pub mod health;
pub mod help;
pub mod install;
pub mod plugins;
pub mod serve;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
```

Add to `Command` enum:

```rust
/// Retired upstream movie collection manager.
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
```

Add to `dispatch` match:

```rust
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
```

- [ ] **Step 5: Run test to verify it passes**

```bash
rtk cargo test -p lab --features retired-upstream -- cli::retired-upstream::tests 2>&1 | tail -10
```

Expected: `test cli::retired-upstream::tests::system_status_args_parse ... ok`

- [ ] **Step 6: Compile check**

```bash
rtk cargo check -p lab --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/lab/src/cli/retired-upstream.rs crates/lab/src/cli.rs
rtk git commit -m "feat(cli): add retired-upstream subcommand as reference CLI shim"
```

---

## Task 7: Create CLI stub subcommands for all 19 remaining services

**Files:**
- Create: 19 CLI stub files
- Modify: `crates/lab/src/cli.rs`

Services `retired-upstream`, `retired-upstream`, `retired-upstream`, `arcane`, `openai` have 25-byte placeholder files — replace them. The other 14 need to be created. All stubs should follow the same pattern and compile correctly.

- [ ] **Step 1: Generate all 19 CLI stub files**

```bash
cd /home/jmagar/workspace/lab

SERVICES=(retired-upstream retired-upstream retired-upstream retired-upstream retired-upstream retired-upstream tailscale linkding memos bytestash paperless arcane unraid unifi retired-upstream gotify openai qdrant tei apprise)

for svc in "${SERVICES[@]}"; do
cat > "crates/lab/src/cli/${svc}.rs" << RUST
//! \`lab ${svc}\` — CLI stub (not yet implemented).
//!
//! Replace this stub once \`${svc}\`'s SDK client is complete.
//! See \`retired-upstream.rs\` for the reference pattern.

use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

use crate::output::OutputFormat;

/// \`lab ${svc}\` arguments.
#[derive(Debug, Args)]
pub struct $( echo "${svc^}" )Args {
    /// Action to run (e.g. help).
    pub action: Option<String>,
}

/// Run the \`lab ${svc}\` subcommand stub.
///
/// # Errors
/// Always returns a "not yet implemented" message.
pub async fn run(_args: $( echo "${svc^}" )Args, _format: OutputFormat) -> Result<ExitCode> {
    anyhow::bail!("${svc} is not yet implemented — run \`lab help\` for available services")
}
RUST
done
echo "Created ${#SERVICES[@]} CLI stubs"
```

Note: the above uses bash parameter expansion for capitalization. Run it in bash. If that expansion doesn't work in your shell, use this Python one-liner to generate the files:

```bash
python3 -c "
import os
services = ['retired-upstream','retired-upstream','retired-upstream','retired-upstream','retired-upstream','retired-upstream','tailscale','linkding','memos','bytestash','paperless','arcane','unraid','unifi','retired-upstream','gotify','openai','qdrant','tei','apprise']
for svc in services:
    cap = svc[0].upper() + svc[1:]
    content = f'''//! \`lab {svc}\` — CLI stub (not yet implemented).
//!
//! Replace this stub once \`{svc}\`'s SDK client is complete.
//! See \`retired-upstream.rs\` for the reference pattern.

use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

use crate::output::OutputFormat;

/// \`lab {svc}\` arguments.
#[derive(Debug, Args)]
pub struct {cap}Args {{
    /// Action to run (e.g. help).
    pub action: Option<String>,
}}

/// Run the \`lab {svc}\` subcommand stub.
///
/// # Errors
/// Always returns a \"not yet implemented\" message.
pub async fn run(_args: {cap}Args, _format: OutputFormat) -> Result<ExitCode> {{
    anyhow::bail!(\"{svc} is not yet implemented — run \`lab help\` for available services\")
}}
'''
    with open(f'crates/lab/src/cli/{svc}.rs', 'w') as f:
        f.write(content)
print(f'Created {len(services)} CLI stubs')
"
```

- [ ] **Step 2: Wire all 19 services in `cli.rs`**

Add to the module declarations in `crates/lab/src/cli.rs`:

```rust
#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
#[cfg(feature = "tailscale")]
pub mod tailscale;
#[cfg(feature = "linkding")]
pub mod linkding;
#[cfg(feature = "memos")]
pub mod memos;
#[cfg(feature = "bytestash")]
pub mod bytestash;
#[cfg(feature = "paperless")]
pub mod paperless;
#[cfg(feature = "arcane")]
pub mod arcane;
#[cfg(feature = "unraid")]
pub mod unraid;
#[cfg(feature = "unifi")]
pub mod unifi;
#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
#[cfg(feature = "gotify")]
pub mod gotify;
#[cfg(feature = "openai")]
pub mod openai;
#[cfg(feature = "qdrant")]
pub mod qdrant;
#[cfg(feature = "tei")]
pub mod tei;
#[cfg(feature = "apprise")]
pub mod apprise;
```

Add to the `Command` enum:

```rust
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
#[cfg(feature = "tailscale")]
Tailscale(tailscale::TailscaleArgs),
#[cfg(feature = "linkding")]
Linkding(linkding::LinkdingArgs),
#[cfg(feature = "memos")]
Memos(memos::MemosArgs),
#[cfg(feature = "bytestash")]
Bytestash(bytestash::BytestashArgs),
#[cfg(feature = "paperless")]
Paperless(paperless::PaperlessArgs),
#[cfg(feature = "arcane")]
Arcane(arcane::ArcaneArgs),
#[cfg(feature = "unraid")]
Unraid(unraid::UnraidArgs),
#[cfg(feature = "unifi")]
Unifi(unifi::UnifiArgs),
#[cfg(feature = "retired-upstream")]
Retired upstream(retired-upstream::Retired upstreamArgs),
#[cfg(feature = "gotify")]
Gotify(gotify::GotifyArgs),
#[cfg(feature = "openai")]
Openai(openai::OpenaiArgs),
#[cfg(feature = "qdrant")]
Qdrant(qdrant::QdrantArgs),
#[cfg(feature = "tei")]
Tei(tei::TeiArgs),
#[cfg(feature = "apprise")]
Apprise(apprise::AppriseArgs),
```

Add to the `dispatch` match:

```rust
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
#[cfg(feature = "tailscale")]
Command::Tailscale(args) => tailscale::run(args, format).await,
#[cfg(feature = "linkding")]
Command::Linkding(args) => linkding::run(args, format).await,
#[cfg(feature = "memos")]
Command::Memos(args) => memos::run(args, format).await,
#[cfg(feature = "bytestash")]
Command::Bytestash(args) => bytestash::run(args, format).await,
#[cfg(feature = "paperless")]
Command::Paperless(args) => paperless::run(args, format).await,
#[cfg(feature = "arcane")]
Command::Arcane(args) => arcane::run(args, format).await,
#[cfg(feature = "unraid")]
Command::Unraid(args) => unraid::run(args, format).await,
#[cfg(feature = "unifi")]
Command::Unifi(args) => unifi::run(args, format).await,
#[cfg(feature = "retired-upstream")]
Command::Retired upstream(args) => retired-upstream::run(args, format).await,
#[cfg(feature = "gotify")]
Command::Gotify(args) => gotify::run(args, format).await,
#[cfg(feature = "openai")]
Command::Openai(args) => openai::run(args, format).await,
#[cfg(feature = "qdrant")]
Command::Qdrant(args) => qdrant::run(args, format).await,
#[cfg(feature = "tei")]
Command::Tei(args) => tei::run(args, format).await,
#[cfg(feature = "apprise")]
Command::Apprise(args) => apprise::run(args, format).await,
```

- [ ] **Step 3: Compile check**

```bash
rtk cargo check -p lab --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors. If there are type-name mismatches (e.g. `ExampleUpstreamArgs` vs `GatewayArgs`), fix the capitalization in the generated stub to match what you put in `cli.rs`.

- [ ] **Step 4: Commit**

```bash
rtk git add crates/lab/src/cli/ crates/lab/src/cli.rs
rtk git commit -m "feat(cli): add stub subcommands for all 20 feature-gated services"
```

---

## Task 8: Fix `doctor.rs` and `health.rs` to be generic over all services

**Files:**
- Modify: `crates/lab/src/cli/doctor.rs`
- Modify: `crates/lab/src/cli/health.rs`

Both currently have a single `#[cfg(feature = "retired-upstream")]` block. They should iterate the same service list that `build_default_registry()` uses. The pattern: iterate a list of `(&'static str, &[EnvVar])` tuples derived from each service's `META`, no per-service cfg blocks needed.

- [ ] **Step 1: Write the failing test for doctor**

Add to `crates/lab/src/cli/doctor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::service_env_checks;

    #[test]
    fn service_env_checks_includes_extract() {
        let checks = service_env_checks();
        assert!(
            checks.iter().any(|(name, _)| *name == "extract"),
            "extract must appear in doctor checks"
        );
    }

    #[test]
    fn service_env_checks_includes_retired-upstream_when_enabled() {
        let checks = service_env_checks();
        #[cfg(feature = "retired-upstream")]
        assert!(
            checks.iter().any(|(name, _)| *name == "retired-upstream"),
            "retired-upstream must appear in doctor checks when retired-upstream feature is enabled"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
rtk cargo test -p lab --all-features -- cli::doctor::tests 2>&1 | tail -10
```

Expected: compile error — `service_env_checks` not defined.

- [ ] **Step 3: Refactor `doctor.rs` to use a service list**

Replace the body of `crates/lab/src/cli/doctor.rs` with:

```rust
//! `lab doctor` — comprehensive health audit.
//!
//! Exit codes: 0 = ok, 1 = warnings, 2 = failures.

use std::process::ExitCode;

use anyhow::Result;
use lab_apis::core::plugin::EnvVar;
use serde::Serialize;

use crate::output::{OutputFormat, print};

/// Severity of a single doctor finding.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// All good.
    Ok,
    /// Non-fatal issue.
    Warn,
    /// Hard failure.
    Fail,
}

/// One entry in the doctor report.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// Service the finding applies to.
    pub service: String,
    /// Check name (e.g., `env_present`, `reachable`).
    pub check: String,
    /// Severity bucket.
    pub severity: Severity,
    /// Human-readable detail.
    pub message: String,
}

/// Full doctor report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// All findings, in scan order.
    pub findings: Vec<Finding>,
}

/// Returns a list of (service_name, required_env_vars) for every enabled service.
/// Extend this list as each service comes online.
pub fn service_env_checks() -> Vec<(&'static str, &'static [EnvVar])> {
    let mut list: Vec<(&'static str, &'static [EnvVar])> = Vec::new();

    // extract is always-on.
    list.push((lab_apis::extract::META.name, lab_apis::extract::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "tailscale")]
    list.push((lab_apis::tailscale::META.name, lab_apis::tailscale::META.required_env));

    #[cfg(feature = "linkding")]
    list.push((lab_apis::linkding::META.name, lab_apis::linkding::META.required_env));

    #[cfg(feature = "memos")]
    list.push((lab_apis::memos::META.name, lab_apis::memos::META.required_env));

    #[cfg(feature = "bytestash")]
    list.push((lab_apis::bytestash::META.name, lab_apis::bytestash::META.required_env));

    #[cfg(feature = "paperless")]
    list.push((lab_apis::paperless::META.name, lab_apis::paperless::META.required_env));

    #[cfg(feature = "arcane")]
    list.push((lab_apis::arcane::META.name, lab_apis::arcane::META.required_env));

    #[cfg(feature = "unraid")]
    list.push((lab_apis::unraid::META.name, lab_apis::unraid::META.required_env));

    #[cfg(feature = "unifi")]
    list.push((lab_apis::unifi::META.name, lab_apis::unifi::META.required_env));

    #[cfg(feature = "retired-upstream")]
    list.push((lab_apis::retired-upstream::META.name, lab_apis::retired-upstream::META.required_env));

    #[cfg(feature = "gotify")]
    list.push((lab_apis::gotify::META.name, lab_apis::gotify::META.required_env));

    #[cfg(feature = "openai")]
    list.push((lab_apis::openai::META.name, lab_apis::openai::META.required_env));

    #[cfg(feature = "qdrant")]
    list.push((lab_apis::qdrant::META.name, lab_apis::qdrant::META.required_env));

    #[cfg(feature = "tei")]
    list.push((lab_apis::tei::META.name, lab_apis::tei::META.required_env));

    #[cfg(feature = "apprise")]
    list.push((lab_apis::apprise::META.name, lab_apis::apprise::META.required_env));

    list
}

/// Run the doctor subcommand.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let mut findings: Vec<Finding> = Vec::new();

    for (service_name, required_env) in service_env_checks() {
        for env in required_env {
            let present = std::env::var(env.name).is_ok();
            findings.push(Finding {
                service: service_name.into(),
                check: format!("env:{}", env.name),
                severity: if present { Severity::Ok } else { Severity::Fail },
                message: if present {
                    format!("{} is set", env.name)
                } else {
                    format!("{} is missing ({})", env.name, env.description)
                },
            });
        }
    }

    let report = Report { findings };
    print(&report, format)?;

    let worst = report.findings.iter().map(|f| f.severity).fold(
        Severity::Ok,
        |acc, s| match (acc, s) {
            (Severity::Fail, _) | (_, Severity::Fail) => Severity::Fail,
            (Severity::Warn, _) | (_, Severity::Warn) => Severity::Warn,
            _ => Severity::Ok,
        },
    );

    Ok(match worst {
        Severity::Ok => ExitCode::SUCCESS,
        Severity::Warn => ExitCode::from(1),
        Severity::Fail => ExitCode::from(2),
    })
}

#[cfg(test)]
mod tests {
    use super::service_env_checks;

    #[test]
    fn service_env_checks_includes_extract() {
        let checks = service_env_checks();
        assert!(
            checks.iter().any(|(name, _)| *name == "extract"),
            "extract must appear in doctor checks"
        );
    }

    #[test]
    fn service_env_checks_includes_retired-upstream_when_enabled() {
        let checks = service_env_checks();
        #[cfg(feature = "retired-upstream")]
        assert!(
            checks.iter().any(|(name, _)| *name == "retired-upstream"),
            "retired-upstream must appear in doctor checks when retired-upstream feature is enabled"
        );
    }
}
```

- [ ] **Step 4: Refactor `health.rs` similarly**

Replace `crates/lab/src/cli/health.rs` with:

```rust
//! `lab health` — quick reachability ping for every configured service.
//!
//! For services without a real client (stubs), health reports "not configured".

use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use crate::output::{OutputFormat, print};

/// One row of the health report.
#[derive(Debug, Clone, Serialize)]
pub struct HealthRow {
    pub service: String,
    pub reachable: bool,
    pub auth_ok: bool,
    pub version: Option<String>,
    pub latency_ms: u64,
    pub message: Option<String>,
}

impl HealthRow {
    fn not_configured(service: &str) -> Self {
        Self {
            service: service.into(),
            reachable: false,
            auth_ok: false,
            version: None,
            latency_ms: 0,
            message: Some("not configured".into()),
        }
    }
}

/// Run the health subcommand.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let mut rows: Vec<HealthRow> = Vec::new();

    rows.push(extract_row().await);

    #[cfg(feature = "retired-upstream")]
    rows.push(retired-upstream_row().await);

    // All other services are stubs — report not-configured until wired.
    // Remove a service from this list when its client_from_env() is implemented.
    for svc in [
        #[cfg(feature = "retired-upstream")] "retired-upstream",
        #[cfg(feature = "retired-upstream")] "retired-upstream",
        #[cfg(feature = "retired-upstream")] "retired-upstream",
        #[cfg(feature = "retired-upstream")] "retired-upstream",
        #[cfg(feature = "retired-upstream")] "retired-upstream",
        #[cfg(feature = "retired-upstream")] "retired-upstream",
        #[cfg(feature = "tailscale")] "tailscale",
        #[cfg(feature = "linkding")] "linkding",
        #[cfg(feature = "memos")] "memos",
        #[cfg(feature = "bytestash")] "bytestash",
        #[cfg(feature = "paperless")] "paperless",
        #[cfg(feature = "arcane")] "arcane",
        #[cfg(feature = "unraid")] "unraid",
        #[cfg(feature = "unifi")] "unifi",
        #[cfg(feature = "retired-upstream")] "retired-upstream",
        #[cfg(feature = "gotify")] "gotify",
        #[cfg(feature = "openai")] "openai",
        #[cfg(feature = "qdrant")] "qdrant",
        #[cfg(feature = "tei")] "tei",
        #[cfg(feature = "apprise")] "apprise",
    ] {
        rows.push(HealthRow::not_configured(svc));
    }

    print(&rows, format)?;
    Ok(ExitCode::SUCCESS)
}

async fn extract_row() -> HealthRow {
    // extract has no network probe — it scans local/SSH paths.
    HealthRow {
        service: "extract".into(),
        reachable: true,
        auth_ok: true,
        version: None,
        latency_ms: 0,
        message: Some("local scan service (always available)".into()),
    }
}

#[cfg(feature = "retired-upstream")]
async fn retired-upstream_row() -> HealthRow {
    use lab_apis::core::ServiceClient;

    let Some(client) = crate::mcp::services::retired-upstream::client_from_env() else {
        return HealthRow {
            service: "retired-upstream".into(),
            reachable: false,
            auth_ok: false,
            version: None,
            latency_ms: 0,
            message: Some("RETIRED_UPSTREAM_URL / RETIRED_UPSTREAM_API_KEY not set".into()),
        };
    };

    match <_ as ServiceClient>::health(&client).await {
        Ok(s) => HealthRow {
            service: "retired-upstream".into(),
            reachable: s.reachable,
            auth_ok: s.auth_ok,
            version: s.version,
            latency_ms: s.latency_ms,
            message: s.message,
        },
        Err(e) => HealthRow {
            service: "retired-upstream".into(),
            reachable: false,
            auth_ok: false,
            version: None,
            latency_ms: 0,
            message: Some(e.to_string()),
        },
    }
}
```

- [ ] **Step 5: Run tests**

```bash
rtk cargo test -p lab --all-features -- cli::doctor::tests 2>&1 | tail -10
```

Expected: both doctor tests pass.

- [ ] **Step 6: Compile check**

```bash
rtk cargo check -p lab --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/lab/src/cli/doctor.rs crates/lab/src/cli/health.rs
rtk git commit -m "feat(cli): make doctor and health generic over all services"
```

---

## Task 9: Overhaul HTTP API to use POST action dispatch

**Files:**
- Create: `crates/lab/src/api/services.rs`
- Create: `crates/lab/src/api/services/extract.rs`
- Create: `crates/lab/src/api/services/retired-upstream.rs`
- Create: `crates/lab/src/api/services/<stub>.rs` — 19 stubs
- Modify: `crates/lab/src/api.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/state.rs`

The current router has `GET /v1/retired-upstream/system/status` — an individual endpoint. The CLAUDE.md specifies `POST /v1/<service>` with `{ "action": "...", "params": {} }` dispatch, mirroring the MCP surface exactly. Rework the router to this shape.

- [ ] **Step 1: Write the failing test**

Add to `crates/lab/src/api/router.rs`:

```rust
#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use axum_test::TestServer;
    use serde_json::json;

    use super::build_router;
    use crate::api::state::AppState;

    #[tokio::test]
    async fn extract_help_returns_200() {
        let app = build_router(AppState::new());
        let server = TestServer::new(app).unwrap();

        let resp = server
            .post("/v1/extract")
            .json(&json!({ "action": "help" }))
            .await;

        assert_eq!(resp.status_code(), StatusCode::OK);
    }
}
```

Add `axum-test` to `crates/lab/Cargo.toml` dev-dependencies:

```toml
[dev-dependencies]
axum-test = "15"
```

- [ ] **Step 2: Run test to verify it fails**

```bash
rtk cargo test -p lab --all-features -- api::router::tests 2>&1 | tail -15
```

Expected: FAIL — no `POST /v1/extract` route.

- [ ] **Step 3: Create the `api/services` module**

Create `crates/lab/src/api/services.rs`:

```rust
//! Per-service HTTP route handlers.
//!
//! Each module exposes a `routes(state: AppState) -> Router` function that
//! mounts a single `POST /` handler dispatching on `action` — identical
//! to the MCP surface. See `extract.rs` for the reference pattern.

pub mod extract;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "tailscale")]
pub mod tailscale;

#[cfg(feature = "linkding")]
pub mod linkding;

#[cfg(feature = "memos")]
pub mod memos;

#[cfg(feature = "bytestash")]
pub mod bytestash;

#[cfg(feature = "paperless")]
pub mod paperless;

#[cfg(feature = "arcane")]
pub mod arcane;

#[cfg(feature = "unraid")]
pub mod unraid;

#[cfg(feature = "unifi")]
pub mod unifi;

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;

#[cfg(feature = "gotify")]
pub mod gotify;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "qdrant")]
pub mod qdrant;

#[cfg(feature = "tei")]
pub mod tei;

#[cfg(feature = "apprise")]
pub mod apprise;
```

- [ ] **Step 4: Create the extract HTTP handler (reference pattern)**

Create `crates/lab/src/api/services/extract.rs`:

```rust
//! HTTP route group for the `extract` service.
//!
//! POST /v1/extract — dispatches to `mcp::services::extract::dispatch`.

use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;
use serde_json::Value;

use super::super::{error::ApiResult, state::AppState};

/// Request body for `POST /v1/extract`.
#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    #[serde(default)]
    pub params: Value,
}

/// Build the extract route group.
#[must_use]
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/", post(handle))
        .with_state(state)
}

async fn handle(
    State(_state): State<AppState>,
    Json(req): Json<ActionRequest>,
) -> ApiResult<Json<Value>> {
    let result = crate::mcp::services::extract::dispatch(&req.action, req.params)
        .await
        .map_err(|e| super::super::error::ApiError::Internal(e.to_string()))?;
    Ok(Json(result))
}
```

- [ ] **Step 5: Create the retired-upstream HTTP handler**

Create `crates/lab/src/api/services/retired-upstream.rs`:

```rust
//! HTTP route group for the `retired-upstream` service.
//!
//! POST /v1/retired-upstream — dispatches to `mcp::services::retired-upstream::dispatch`.

use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;
use serde_json::Value;

use super::super::{error::ApiResult, state::AppState};

/// Request body for `POST /v1/retired-upstream`.
#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    #[serde(default)]
    pub params: Value,
}

/// Build the retired-upstream route group.
#[must_use]
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/", post(handle))
        .with_state(state)
}

async fn handle(
    State(_state): State<AppState>,
    Json(req): Json<ActionRequest>,
) -> ApiResult<Json<Value>> {
    let result = crate::mcp::services::retired-upstream::dispatch(&req.action, req.params)
        .await
        .map_err(|e| super::super::error::ApiError::Internal(e.to_string()))?;
    Ok(Json(result))
}
```

- [ ] **Step 6: Generate 19 stub HTTP handler files**

```bash
cd /home/jmagar/workspace/lab
mkdir -p crates/lab/src/api/services

SERVICES=(retired-upstream retired-upstream retired-upstream retired-upstream retired-upstream retired-upstream tailscale linkding memos bytestash paperless arcane unraid unifi retired-upstream gotify openai qdrant tei apprise)

for svc in "${SERVICES[@]}"; do
cat > "crates/lab/src/api/services/${svc}.rs" << RUST
//! HTTP route group stub for the \`${svc}\` service.

use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;
use serde_json::Value;

use super::super::{error::{ApiError, ApiResult}, state::AppState};

#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    #[serde(default)]
    pub params: Value,
}

#[must_use]
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/", post(handle))
        .with_state(state)
}

async fn handle(
    State(_state): State<AppState>,
    Json(req): Json<ActionRequest>,
) -> ApiResult<Json<Value>> {
    let result = crate::mcp::services::${svc}::dispatch(&req.action, req.params)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(result))
}
RUST
done
echo "Created ${#SERVICES[@]} HTTP handler stubs"
```

- [ ] **Step 7: Add `ApiError::Internal` variant if missing**

Check `crates/lab/src/api/error.rs` — if `Internal(String)` isn't already a variant, add it:

```rust
/// Catch-all for unexpected internal errors.
Internal(String),
```

And in the `IntoResponse` impl, map it to 500:

```rust
ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR,
    Json(serde_json::json!({ "kind": "internal_error", "message": msg }))).into_response(),
```

- [ ] **Step 8: Declare `services` in `api.rs`**

In `crates/lab/src/api.rs`, add:

```rust
pub mod services;
```

- [ ] **Step 9: Overhaul `router.rs` to mount all service route groups**

Replace the contents of `crates/lab/src/api/router.rs` with:

```rust
//! Top-level axum router builder.
//!
//! Mounts `POST /v1/<service>` for every feature-enabled service, plus
//! `/health` and `/ready`. The action dispatch is identical in shape to
//! the MCP surface — clients can share logic across transports.

use std::time::Duration;

use axum::{Router, routing::get};
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, timeout::TimeoutLayer, trace::TraceLayer,
};

use super::{health, services, state::AppState};

/// Build the full `lab` HTTP router.
#[must_use]
pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready));

    // extract is always-on.
    router = router.nest("/v1/extract", services::extract::routes(state.clone()));

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "tailscale")]
    {
        router = router.nest("/v1/tailscale", services::tailscale::routes(state.clone()));
    }

    #[cfg(feature = "linkding")]
    {
        router = router.nest("/v1/linkding", services::linkding::routes(state.clone()));
    }

    #[cfg(feature = "memos")]
    {
        router = router.nest("/v1/memos", services::memos::routes(state.clone()));
    }

    #[cfg(feature = "bytestash")]
    {
        router = router.nest("/v1/bytestash", services::bytestash::routes(state.clone()));
    }

    #[cfg(feature = "paperless")]
    {
        router = router.nest("/v1/paperless", services::paperless::routes(state.clone()));
    }

    #[cfg(feature = "arcane")]
    {
        router = router.nest("/v1/arcane", services::arcane::routes(state.clone()));
    }

    #[cfg(feature = "unraid")]
    {
        router = router.nest("/v1/unraid", services::unraid::routes(state.clone()));
    }

    #[cfg(feature = "unifi")]
    {
        router = router.nest("/v1/unifi", services::unifi::routes(state.clone()));
    }

    #[cfg(feature = "retired-upstream")]
    {
        router = router.nest("/v1/retired-upstream", services::retired-upstream::routes(state.clone()));
    }

    #[cfg(feature = "gotify")]
    {
        router = router.nest("/v1/gotify", services::gotify::routes(state.clone()));
    }

    #[cfg(feature = "openai")]
    {
        router = router.nest("/v1/openai", services::openai::routes(state.clone()));
    }

    #[cfg(feature = "qdrant")]
    {
        router = router.nest("/v1/qdrant", services::qdrant::routes(state.clone()));
    }

    #[cfg(feature = "tei")]
    {
        router = router.nest("/v1/tei", services::tei::routes(state.clone()));
    }

    #[cfg(feature = "apprise")]
    {
        router = router.nest("/v1/apprise", services::apprise::routes(state.clone()));
    }

    router
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use super::build_router;
    use crate::api::state::AppState;

    #[tokio::test]
    async fn extract_help_returns_200() {
        use axum_test::TestServer;

        let app = build_router(AppState::new());
        let server = TestServer::new(app).unwrap();

        let resp = server
            .post("/v1/extract")
            .json(&json!({ "action": "help" }))
            .await;

        assert_eq!(resp.status_code(), StatusCode::OK);
    }
}
```

- [ ] **Step 10: Simplify `AppState` — remove Retired upstream-specific field**

The API handlers now call `mcp::services::<s>::dispatch()` directly (which calls `client_from_env()` inline), so `AppState` doesn't need to hold individual service clients. Replace `crates/lab/src/api/state.rs` with:

```rust
//! Shared application state for axum handlers.
//!
//! Intentionally minimal — service clients are constructed per-request
//! inside each handler's MCP dispatch call via `client_from_env()`.
//! If connection pooling or client reuse becomes necessary, add service
//! clients back here at that time.

/// Application state passed to every axum handler via `State<AppState>`.
///
/// Currently a unit type; expands when shared resources (e.g. a DB pool)
/// are needed.
#[derive(Clone, Default)]
pub struct AppState;

impl AppState {
    /// Construct application state.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}
```

- [ ] **Step 11: Run tests**

```bash
rtk cargo test -p lab --all-features -- api::router::tests 2>&1 | tail -15
```

Expected: `test api::router::tests::extract_help_returns_200 ... ok`

- [ ] **Step 12: Compile check**

```bash
rtk cargo check -p lab --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Step 13: Commit**

```bash
rtk git add \
  crates/lab/src/api.rs \
  crates/lab/src/api/services.rs \
  crates/lab/src/api/services/ \
  crates/lab/src/api/router.rs \
  crates/lab/src/api/state.rs \
  crates/lab/src/api/error.rs
rtk git commit -m "feat(api): overhaul HTTP router to POST /v1/<service> action dispatch for all services"
```

---

## Final Verification

- [ ] **Full test suite**

```bash
rtk cargo test --workspace --all-features 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Full compile with all features**

```bash
rtk cargo build --workspace --all-features 2>&1 | grep "^error" | head -20
```

Expected: no errors.

- [ ] **Smoke test `lab help`**

```bash
cargo run --all-features -- help 2>&1 | head -30
```

Expected: all 21 services listed with their categories.

- [ ] **Smoke test `lab serve` with a help call**

```bash
echo '{"service":"extract","action":"help","params":{}}' | cargo run --all-features -- serve --transport stdio
```

Expected: JSON response with extract's action catalog.

- [ ] **Create PR**

```bash
rtk git push -u origin feat/lab-operational
gh pr create --title "feat: core CLI completion — all 21 services wired, doctor/health generic, HTTP POST dispatch" \
  --body "Completes the core CLI infrastructure so adding a new service requires only implementing it:
- All 21 services registered in registry, dispatch, catalog, CLI, and HTTP API
- extract migrated to real ActionSpec types from core
- doctor and health iterate services generically from META
- HTTP API overhauled from individual endpoint routes to POST /v1/<service> action dispatch
- AppState simplified (clients constructed per-request via client_from_env)
- Retired upstream CLI shim added as reference implementation"
```
