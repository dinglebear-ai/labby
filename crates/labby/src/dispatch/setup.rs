//! Shared dispatch layer for the `setup` Bootstrap orchestrator.
//!
//! `setup` is a synthetic Bootstrap service: no external service URL, no
//! feature gate. All fs I/O lives here (per `labby-apis` SDK purity rule).
//! `setup.draft.commit` invokes `doctor.audit.full` inline; that is the
//! single sanctioned cross-service dispatch call (see the orchestrator
//! exception clause in `crates/labby/src/dispatch/CLAUDE.md`).

pub(crate) mod access_bootstrap;
mod bootstrap;
mod caller;
mod catalog;
pub(crate) mod claude_plugins;
mod client;
mod constrained_yaml;
mod dispatch;
mod draft;
pub(crate) mod host_service;
pub(crate) mod incus;
pub(crate) mod organization_profile;
pub(crate) mod owner_link;
mod params;
mod plugin_hook;
pub(crate) mod provision;
pub(crate) mod proxy;
pub(crate) mod public_proxy;
mod secret_mask;
mod secure_file;
mod settings;
mod state;
pub(crate) mod tailscale_funnel;
mod token;
mod types;

pub use access_bootstrap::{
    cleanup_prepare, complete_prepare, consume_prepare, inspect_prepare, prepare_access_bootstrap,
    recover_prepare, revoke_prepare, status_prepare,
};
pub(crate) use bootstrap::bootstrap_at;
pub use bootstrap::{
    BootstrapOutcome, bootstrap, bootstrap_action, ensure_oauth_encryption_key_at, should_bootstrap,
};
pub use caller::{SetupCaller, SetupCallerEvidence};
pub use catalog::{ACTIONS, LOCAL_ONLY_ACTIONS, PLUGIN_LIFECYCLE_ACTIONS};
pub(crate) use client::draft_path as setup_draft_path;
pub use dispatch::{dispatch, dispatch_for_caller};
pub(crate) use draft::{
    discard as discard_setup_draft, merge_entries as merge_setup_draft_entries,
    read_entries as read_setup_draft_entries,
};
pub use token::generate_mcp_token;
pub use types::{
    AccessBootstrapManifest, AccessBootstrapPrepare, AccessBootstrapPrepareOutcome, CommitOutcome,
    DraftEntry, PrepareJournal, PrepareJournalState, SECRET_SENTINEL, SetupClient, SetupSnapshot,
    SetupState,
};

use labby_primitives::plugin::{Category, EnvVar, PluginMeta};

const OPTIONAL_ENV: &[EnvVar] = &[
    EnvVar {
        name: "LABBY_ACCESS_MIGRATION_EVIDENCE",
        description: "Path to the source/checkpoint-bound approval evidence for an access-store schema migration",
        example: "/run/labby/access-migration-v7.json",
        secret: false,
        ui: None,
    },
    EnvVar {
        name: crate::config::depot::DEFAULT_AUTHORITY_BEARER_TOKEN_ENV,
        description: "Bearer token presented to Depot's managed authority inbox (default variable for depot.authority_bearer_token_env)",
        example: "<depot-authority-bearer>",
        secret: true,
        ui: None,
    },
    EnvVar {
        name: crate::config::depot::DEFAULT_AUTHORITY_SIGNING_KEY_ENV,
        description: "Active Ed25519 signing seed (base64url, no padding, 32 bytes) for Depot projection envelopes, delegated assertions, and organization bootstrap profiles",
        example: "<base64url-ed25519-seed>",
        secret: true,
        ui: None,
    },
    EnvVar {
        name: organization_profile::TEAM_DEPOT_URL_ENV,
        description: "Externally reachable HTTPS Team Depot MCP endpoint offered to newly enrolled teammates",
        example: "https://team.example/mcp",
        secret: false,
        ui: None,
    },
    EnvVar {
        name: organization_profile::SIGNING_KEY_ID_ENV,
        description: "Public key identifier stamped into organization bootstrap profiles",
        example: "team-bootstrap-2026-09",
        secret: false,
        ui: None,
    },
];

/// Compile-time metadata for the setup Bootstrap service.
pub const META: PluginMeta = PluginMeta {
    name: "setup",
    display_name: "Setup",
    description: "First-run + draft-commit configuration flow",
    category: Category::Bootstrap,
    docs_url: "",
    required_env: &[],
    optional_env: OPTIONAL_ENV,
    default_port: None,
    supports_multi_instance: false,
};
