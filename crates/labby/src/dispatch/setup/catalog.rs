//! Action catalog for the `setup` Bootstrap orchestrator.

use labby_primitives::action::{ActionSpec, ParamSpec};

/// Plugin-lifecycle action names — canonical dotted forms paired with their
/// deprecated snake_case aliases. **Single source of truth** for the HTTP
/// loopback restriction enforced in `crate::api::services::setup`.
///
/// Invariant: every name here MUST have (a) a catalog `ActionSpec` below and
/// (b) a dispatch arm in `dispatch.rs`. The gate consumes this list directly,
/// so a name that the dispatcher can route but that is missing here would be a
/// loopback-restriction bypass. The `plugin_lifecycle_actions_*` tests enforce
/// the catalog membership and the dispatch routing so the three locations
/// cannot silently drift.
///
/// Pairs are ordered (canonical, alias) so tests can assert metadata parity.
pub const PLUGIN_LIFECYCLE_ACTIONS: &[&str] = &[
    "plugins.installed",
    "installed_plugins",
    "services.status",
    "services_status",
    "plugin.install",
    "install_plugin",
    "plugin.uninstall",
    "uninstall_plugin",
];

/// Setup actions that may only run from a trusted local transport. These
/// mint first-run credentials, initiate host-local connectivity probes, or
/// mutate host-local proxy/Tailscale state, so an admin bearer alone is not
/// sufficient.
pub const LOCAL_ONLY_ACTIONS: &[&str] = &[
    "bootstrap",
    "plugin_connectivity",
    "proxy.configure",
    "tailscale_funnel.configure",
    "tailscale_funnel.disable",
];

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        returns: "Catalog",
        params: &[],
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        returns: "Schema",
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
    },
    ActionSpec {
        name: "state",
        description: "First-run + draft snapshot for the wizard / settings UI",
        destructive: false,
        requires_admin: true,
        returns: "SetupSnapshot",
        params: &[],
    },
    ActionSpec {
        name: "bootstrap",
        description: "Create ~/.labby/.env with a generated token + loopback defaults when absent (first-run)",
        destructive: true,
        requires_admin: true,
        returns: "BootstrapOutcome",
        params: &[],
    },
    ActionSpec {
        name: "schema.get",
        description: "UiSchema projection for all (or filtered) services",
        destructive: false,
        requires_admin: false,
        returns: "ServiceSchemaMap",
        params: &[ParamSpec {
            name: "services",
            ty: "string[]",
            required: false,
            description: "Optional filter; defaults to every service in the registry",
        }],
    },
    ActionSpec {
        name: "draft.get",
        description: "Read .env.draft with secret values masked to '***'",
        destructive: false,
        requires_admin: true,
        returns: "DraftEntry[]",
        params: &[],
    },
    ActionSpec {
        name: "draft.set",
        description: "Write a key (or section) into .env.draft (validated server-side)",
        destructive: true,
        requires_admin: true,
        returns: "DraftSetOutcome",
        params: &[
            ParamSpec {
                name: "entries",
                ty: "DraftEntry[]",
                required: true,
                description: "Key/value pairs to write into the draft",
            },
            ParamSpec {
                name: "force",
                ty: "boolean",
                required: false,
                description: "Overwrite conflicting draft keys (default false)",
            },
        ],
    },
    ActionSpec {
        name: "draft.discard",
        description: "Discard .env.draft without modifying .env",
        destructive: true,
        requires_admin: true,
        returns: "DraftDiscardOutcome",
        params: &[],
    },
    ActionSpec {
        name: "draft.commit",
        description: "Run audit and atomically merge .env.draft into .env",
        destructive: true,
        requires_admin: true,
        returns: "CommitOutcome",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Overwrite conflicting .env keys (default false)",
        }],
    },
    ActionSpec {
        name: "settings.state",
        description: "Return section-scoped safe settings values and source metadata",
        destructive: false,
        requires_admin: true,
        returns: "SettingsState",
        params: &[ParamSpec {
            name: "section",
            ty: "string",
            required: false,
            description: "Settings section id; defaults to core",
        }],
    },
    ActionSpec {
        name: "settings.schema",
        description: "Return the safe settings schema with risk and write-policy metadata",
        destructive: false,
        requires_admin: false,
        returns: "SettingsSchema",
        params: &[],
    },
    ActionSpec {
        name: "settings.env_schema",
        description: "Return generated and registry-derived environment variable inventory",
        destructive: false,
        requires_admin: false,
        returns: "EnvSettingSpec[]",
        params: &[],
    },
    ActionSpec {
        name: "settings.advanced_state",
        description: "Return redacted advanced settings state",
        destructive: false,
        requires_admin: true,
        returns: "SettingsState",
        params: &[],
    },
    ActionSpec {
        name: "settings.update",
        description: "Update non-secret operator settings with validation",
        destructive: true,
        requires_admin: true,
        returns: "SettingsState",
        params: &[ParamSpec {
            name: "services.built_in_upstream_apis_enabled",
            ty: "boolean",
            required: true,
            description: "Enable built-in upstream API service integrations",
        }],
    },
    ActionSpec {
        name: "settings.config.update",
        description: "Admin-only scalar config.toml settings update",
        destructive: true,
        requires_admin: true,
        returns: "SettingsMutationOutcome",
        params: &[ParamSpec {
            name: "entries",
            ty: "SettingsUpdateEntry[]",
            required: true,
            description: "Schema-approved config scalar updates",
        }],
    },
    ActionSpec {
        name: "settings.env.update",
        description: "Admin-only targeted .env settings update for known low-risk LABBY_* keys",
        destructive: true,
        requires_admin: true,
        returns: "SettingsState",
        params: &[ParamSpec {
            name: "entries",
            ty: "SettingsUpdateEntry[]",
            required: true,
            description: "Schema-approved env scalar updates",
        }],
    },
    ActionSpec {
        name: "plugin_hook",
        description: "Run binary-owned local plugin setup checks; in repair mode also syncs CLAUDE_PLUGIN_OPTION_* and probes server connectivity",
        destructive: true,
        requires_admin: true,
        // Composite payload: { setup: SetupReport, sync: PluginSyncOutcome|null, connectivity: ConnectivityOutcome }.
        // `sync` is null when called with repair=false (check mode is guaranteed non-mutating).
        returns: "PluginHookReport",
        params: &[ParamSpec {
            name: "repair",
            ty: "boolean",
            required: false,
            description: "Create missing local Lab setup files and sync plugin env; defaults to true",
        }],
    },
    ActionSpec {
        name: "plugin_sync",
        description: "Sync CLAUDE_PLUGIN_OPTION_* env vars into ~/.labby/.env as LABBY_* vars",
        destructive: true,
        requires_admin: true,
        returns: "PluginSyncOutcome",
        params: &[],
    },
    ActionSpec {
        name: "plugin_export",
        description: "Read ~/.labby/.env and return current values keyed by userConfig field name",
        destructive: false,
        requires_admin: true,
        returns: "PluginExportOutcome",
        params: &[],
    },
    ActionSpec {
        name: "plugin_connectivity",
        description: "Validate connectivity to the lab MCP server at {server_url}/health",
        destructive: false,
        requires_admin: true,
        returns: "ConnectivityOutcome",
        params: &[ParamSpec {
            name: "server_url",
            ty: "string",
            required: false,
            description: "Requested server URL; it must match the active target selected from CLAUDE_PLUGIN_OPTION_SERVER_URL, LABBY_SERVER_URL, or the localhost:40100 host-proxy default",
        }],
    },
    ActionSpec {
        name: "check",
        description: "Check local Lab setup prerequisites without mutating the filesystem",
        destructive: false,
        requires_admin: false,
        returns: "SetupReport",
        params: &[],
    },
    ActionSpec {
        name: "repair",
        description: "Repair missing local Lab setup prerequisites without contacting external services",
        destructive: true,
        requires_admin: true,
        returns: "SetupReport",
        params: &[],
    },
    ActionSpec {
        name: "proxy.configure",
        description: "Persist local stdio-proxy defaults and securely store a bearer secret when required",
        destructive: true,
        requires_admin: true,
        returns: "ProxySetupOutcome",
        params: &[
            ParamSpec {
                name: "preferences",
                ty: "ProxyPreferences",
                required: true,
                description: "Validated non-secret proxy preferences to persist in config.toml",
            },
            ParamSpec {
                name: "bearer_token",
                ty: "string",
                required: false,
                description: "Write-only bearer token read from stdin; never returned or logged",
            },
            ParamSpec {
                name: "dry_run",
                ty: "boolean",
                required: false,
                description: "Preview whether config or secret files would change without mutation",
            },
        ],
    },
    ActionSpec {
        name: "tailscale_funnel.inspect",
        description: "Inspect whether this host can expose Labby through Tailscale Funnel without mutating Tailscale",
        destructive: false,
        requires_admin: false,
        returns: "TailscaleFunnelInspection",
        params: &[ParamSpec {
            name: "https_port",
            ty: "integer",
            required: false,
            description: "Public Funnel HTTPS port; one of 443, 8443, or 10000 (default 443)",
        }],
    },
    ActionSpec {
        name: "tailscale_funnel.configure",
        description: "Safely expose the local Labby HTTP server through Tailscale Funnel without replacing an existing mapping",
        destructive: true,
        requires_admin: true,
        returns: "TailscaleFunnelMutationOutcome",
        params: &[
            ParamSpec {
                name: "backend_url",
                ty: "string",
                required: false,
                description: "Loopback Labby backend origin; defaults to http://127.0.0.1:8765",
            },
            ParamSpec {
                name: "https_port",
                ty: "integer",
                required: false,
                description: "Public Funnel HTTPS port; one of 443, 8443, or 10000 (default 443)",
            },
        ],
    },
    ActionSpec {
        name: "tailscale_funnel.disable",
        description: "Disable only the Tailscale Funnel mapping that exactly matches the expected local Labby backend",
        destructive: true,
        requires_admin: true,
        returns: "TailscaleFunnelMutationOutcome",
        params: &[
            ParamSpec {
                name: "backend_url",
                ty: "string",
                required: false,
                description: "Expected loopback Labby backend origin; defaults to http://127.0.0.1:8765",
            },
            ParamSpec {
                name: "https_port",
                ty: "integer",
                required: false,
                description: "Public Funnel HTTPS port; one of 443, 8443, or 10000 (default 443)",
            },
        ],
    },
    ActionSpec {
        name: "public_proxy.render",
        description: "Render validated Caddy, Nginx, or Traefik configuration for a public Labby HTTPS origin",
        destructive: false,
        requires_admin: false,
        returns: "PublicProxyRenderOutcome",
        params: &[
            ParamSpec {
                name: "public_url",
                ty: "string",
                required: true,
                description: "Browser-visible HTTPS Labby origin",
            },
            ParamSpec {
                name: "backend_url",
                ty: "string",
                required: false,
                description: "Private Labby backend origin; defaults to http://127.0.0.1:8765",
            },
            ParamSpec {
                name: "format",
                ty: "caddy|nginx|traefik|all",
                required: false,
                description: "Proxy configuration format; defaults to all for API/WebUI callers",
            },
        ],
    },
    ActionSpec {
        name: "organization_profile.create",
        description: "Create a signed non-secret organization bootstrap profile for personal Labby",
        destructive: false,
        requires_admin: true,
        returns: "OrganizationBootstrapProfile",
        params: &[
            ParamSpec {
                name: "organization_id",
                ty: "string",
                required: true,
                description: "Organization identifier embedded in the signed profile",
            },
            ParamSpec {
                name: "team_depot_url",
                ty: "string",
                required: true,
                description: "Externally reachable HTTPS MCP endpoint for Team Depot",
            },
            ParamSpec {
                name: "key_id",
                ty: "string",
                required: true,
                description: "Public signing-key identifier embedded in the profile",
            },
            ParamSpec {
                name: "signing_key_env",
                ty: "string",
                required: false,
                description: "Optional secret environment reference; defaults to LABBY_DEPOT_AUTHORITY_SIGNING_KEY",
            },
        ],
    },
    ActionSpec {
        name: "organization_profile.preview",
        description: "Verify and preview a signed organization profile without mutating personal Labby",
        destructive: false,
        requires_admin: false,
        returns: "OrganizationBootstrapPreview",
        params: &[ParamSpec {
            name: "profile",
            ty: "object",
            required: true,
            description: "Signed organization bootstrap profile",
        }],
    },
    ActionSpec {
        name: "organization_profile.apply",
        description: "Verify and atomically import approved organization integrations into personal Labby",
        destructive: true,
        requires_admin: true,
        returns: "OrganizationBootstrapApplyOutcome",
        params: &[
            ParamSpec {
                name: "profile",
                ty: "object",
                required: true,
                description: "Signed organization bootstrap profile",
            },
            ParamSpec {
                name: "expected_signer_fingerprint",
                ty: "string",
                required: true,
                description: "Pinned SHA-256 fingerprint of the trusted organization profile signing key",
            },
        ],
    },
    // -- Plugin-lifecycle actions ------------------------------------------
    //
    // These actions are HTTP loopback-gated in
    // `crate::api::services::setup::plugin_lifecycle_action`, which reads its
    // name set from `PLUGIN_LIFECYCLE_ACTIONS` above. The canonical names are
    // the dotted `<resource>.<verb>` forms below; the snake_case entries that
    // follow each one are deprecated aliases retained only for backward
    // compatibility with external callers using the historical names — no
    // in-tree caller depends on them (the CLI uses the dotted forms). Both
    // forms route to the same handler in `dispatch.rs`. Every name in
    // `PLUGIN_LIFECYCLE_ACTIONS` must have an entry here and a dispatch arm;
    // the `plugin_lifecycle_actions_*` tests enforce that lockstep.
    ActionSpec {
        name: "plugins.installed",
        description: "List installed Claude Code lab plugins",
        destructive: false,
        requires_admin: true,
        returns: "InstalledPlugin[]",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Bypass the short in-process cache",
        }],
    },
    // Deprecated alias for `plugins.installed`.
    ActionSpec {
        name: "installed_plugins",
        description: "Deprecated alias for `plugins.installed`",
        destructive: false,
        requires_admin: true,
        returns: "InstalledPlugin[]",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Bypass the short in-process cache",
        }],
    },
    ActionSpec {
        name: "services.status",
        description: "Join service configuration, draft, and Claude plugin state",
        destructive: false,
        requires_admin: true,
        returns: "ServiceStatus[]",
        params: &[],
    },
    // Deprecated alias for `services.status`.
    ActionSpec {
        name: "services_status",
        description: "Deprecated alias for `services.status`",
        destructive: false,
        requires_admin: true,
        returns: "ServiceStatus[]",
        params: &[],
    },
    ActionSpec {
        name: "plugin.install",
        description: "Install the Claude Code plugin for one configured service",
        destructive: true,
        requires_admin: true,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    // Deprecated alias for `plugin.install`.
    ActionSpec {
        name: "install_plugin",
        description: "Deprecated alias for `plugin.install`",
        destructive: true,
        requires_admin: true,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    ActionSpec {
        name: "plugin.uninstall",
        description: "Uninstall the Claude Code plugin for one service",
        destructive: true,
        requires_admin: true,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    // Deprecated alias for `plugin.uninstall`.
    ActionSpec {
        name: "uninstall_plugin",
        description: "Deprecated alias for `plugin.uninstall`",
        destructive: true,
        requires_admin: true,
        returns: "PluginMutationResult",
        params: &[ParamSpec {
            name: "service",
            ty: "string",
            required: true,
            description: "Registered service name",
        }],
    },
    ActionSpec {
        name: "finalize",
        description: "Alias for draft.commit; same params, same returns",
        destructive: true,
        requires_admin: true,
        returns: "CommitOutcome",
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Overwrite conflicting .env keys (default false)",
        }],
    },
];
