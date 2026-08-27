use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct GatewayArgs {
    #[command(subcommand)]
    pub command: GatewayCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayCommand {
    /// List configured gateways and their runtime status.
    List,
    /// Get one configured gateway.
    Get(GatewayGetArgs),
    /// Test a configured or proposed gateway without saving it.
    Test(GatewayTestArgs),
    /// Add a gateway and reconcile runtime state.
    Add(GatewayAddArgs),
    /// Update a gateway and reconcile runtime state.
    Update(GatewayUpdateArgs),
    /// Remove a gateway and reconcile runtime state.
    Remove(GatewayRemoveArgs),
    /// Manage Lab-backed virtual servers quarantined during config migration.
    Quarantine(GatewayQuarantineArgs),
    /// Manage public MCP routes protected by Lab OAuth.
    ProtectedRoute(GatewayProtectedRouteArgs),
    /// Manage reusable gateway capability loadouts.
    Loadout(GatewayLoadoutArgs),
    /// Reload gateways from config and reconcile runtime state.
    Reload,
    /// Manage upstream MCP server lifecycle and OAuth.
    Mcp(GatewayMcpArgs),
    /// Inspect inbound MCP clients/sessions connected to this gateway.
    Clients(GatewayClientsArgs),
    /// Scan the machine for MCP server configs from known editors and tools (read-only)
    Discover(GatewayDiscoverArgs),
    /// Import discovered MCP servers into the gateway (disabled by default)
    Import(GatewayImportArgs),
    /// Manage pending discovered servers waiting for approval
    Pending(GatewayPendingArgs),
    /// Show resolved public URL configuration (app and MCP gateway)
    PublicUrls,
    /// Search, inspect, and execute Code Mode snippets through dispatch
    Code(GatewayCodeArgs),
    /// Generate and approve Code Mode upstream hint proposals.
    Enrich(GatewayEnrichArgs),
    /// Inspect and manage Agent Skills exposed by gateway upstreams.
    Skills(GatewaySkillsArgs),
    /// Query gateway upstream call-usage telemetry.
    Usage(GatewayUsageArgs),
}

#[derive(Debug, Args)]
pub struct GatewayLoadoutArgs {
    #[command(subcommand)]
    pub command: GatewayLoadoutCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayLoadoutCommand {
    /// List configured Loadouts.
    List,
    /// Get one Loadout.
    Get(GatewayLoadoutNameArgs),
    /// Add a reusable Loadout.
    Add(GatewayLoadoutCreateArgs),
    /// Patch selected Loadout fields without resetting unspecified fields.
    Update(GatewayLoadoutUpdateArgs),
    /// Remove an unreferenced Loadout.
    Remove(GatewayLoadoutRemoveArgs),
}

#[derive(Debug, Args)]
pub struct GatewayLoadoutNameArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct GatewayLoadoutRemoveArgs {
    pub name: String,
    /// Stage removal for restart after protected-route references have been staged away.
    #[arg(long)]
    pub stage_for_restart: bool,
}

#[derive(Debug, Args)]
pub struct GatewayLoadoutCreateArgs {
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    /// Upstream names selected by this Loadout. Repeat or comma-separate.
    #[arg(long = "upstream", value_delimiter = ',')]
    pub upstreams: Vec<String>,
    /// Built-in Lab services selected by this Loadout. Repeat or comma-separate.
    #[arg(long = "service", value_delimiter = ',')]
    pub services: Vec<String>,
    /// Hide direct MCP Tools on this Loadout.
    #[arg(long)]
    pub no_tools: bool,
    /// Hide MCP Resources on this Loadout. Skills require Resources.
    #[arg(long)]
    pub no_resources: bool,
    /// Hide MCP Prompts on this Loadout.
    #[arg(long)]
    pub no_prompts: bool,
    /// Hide Agent Skills on this Loadout.
    #[arg(long)]
    pub no_skills: bool,
    /// Expose Code Mode on this Loadout.
    #[arg(long)]
    pub code_mode: bool,
}

#[derive(Debug, Args)]
pub struct GatewayLoadoutUpdateArgs {
    pub name: String,
    #[arg(long)]
    pub new_name: Option<String>,
    #[arg(long, conflicts_with = "clear_description")]
    pub description: Option<String>,
    #[arg(long, conflicts_with = "description")]
    pub clear_description: bool,
    /// Replace upstream selection. Repeat or comma-separate.
    #[arg(
        long = "upstream",
        value_delimiter = ',',
        conflicts_with = "clear_upstreams"
    )]
    pub upstreams: Vec<String>,
    /// Clear all selected upstreams.
    #[arg(long, conflicts_with = "upstreams")]
    pub clear_upstreams: bool,
    /// Replace service selection. Repeat or comma-separate.
    #[arg(
        long = "service",
        value_delimiter = ',',
        conflicts_with = "clear_services"
    )]
    pub services: Vec<String>,
    /// Clear all selected built-in services.
    #[arg(long, conflicts_with = "services")]
    pub clear_services: bool,
    #[arg(long, action = clap::ArgAction::Set)]
    pub expose_tools: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set)]
    pub expose_resources: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set)]
    pub expose_prompts: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set)]
    pub expose_skills: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set)]
    pub expose_code_mode: Option<bool>,
    /// Stage this Loadout patch for the next Labby restart. Required when the Loadout is mounted by an enabled protected route.
    #[arg(long)]
    pub stage_for_restart: bool,
}

#[derive(Debug, Args)]
pub struct GatewaySkillsArgs {
    #[command(subcommand)]
    pub command: GatewaySkillsCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewaySkillsCommand {
    /// List Skills support, trust state, validation results, and exposure.
    List(GatewaySkillsListArgs),
    /// Trust an upstream's Agent Skills and allow Labby to enumerate them.
    Trust(GatewaySkillsTrustArgs),
    /// Stop trusting an upstream's Agent Skills.
    Untrust(GatewaySkillsUpstreamArgs),
    /// Replace the skill-name exposure allowlist. Repeat --pattern for multiple patterns.
    Expose(GatewaySkillsExposeArgs),
    /// Clear the skill exposure allowlist so every validated skill may be exposed.
    ExposeAll(GatewaySkillsUpstreamArgs),
}

#[derive(Debug, Args)]
pub struct GatewaySkillsListArgs {
    /// Limit the operator view to one upstream.
    #[arg(long)]
    pub upstream: Option<String>,
}

#[derive(Debug, Args)]
pub struct GatewaySkillsUpstreamArgs {
    pub upstream: String,
}

#[derive(Debug, Args)]
pub struct GatewaySkillsTrustArgs {
    pub upstream: String,
    /// Skip the trust confirmation prompt.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct GatewaySkillsExposeArgs {
    pub upstream: String,
    /// Exact skill name or wildcard pattern. Repeat to build the allowlist.
    #[arg(long = "pattern", required = true)]
    pub patterns: Vec<String>,
}

#[derive(Debug, Args)]
pub struct GatewayUsageArgs {
    #[command(subcommand)]
    pub command: GatewayUsageCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayUsageCommand {
    /// Aggregated totals, error rate, top tools, top actors.
    Metrics(GatewayUsageMetricsArgs),
    /// Raw call records, newest first.
    Calls(GatewayUsageCallsArgs),
}

#[derive(Debug, Args)]
pub struct GatewayUsageMetricsArgs {
    #[arg(long)]
    pub since_unix: Option<i64>,
    #[arg(long)]
    pub until_unix: Option<i64>,
    #[arg(long)]
    pub upstream: Option<String>,
    /// Restrict to one qualified upstream::tool target.
    #[arg(long)]
    pub tool: Option<String>,
    /// Restrict to one capability family.
    #[arg(long)]
    pub capability: Option<String>,
    /// Restrict to one operation name.
    #[arg(long)]
    pub operation: Option<String>,
    /// Restrict by OAuth subject scoping (`true` or `false`).
    #[arg(long)]
    pub subject_scoped: Option<bool>,
    /// Restrict to one actor subject.
    #[arg(long)]
    pub actor: Option<String>,
    /// Restrict to one outcome; failed matches every non-ok outcome.
    #[arg(long)]
    pub outcome: Option<String>,
    /// Case-insensitive search across target, operation, actor, and outcome.
    #[arg(long)]
    pub search: Option<String>,
    /// Return this many complete-window time buckets (max 168).
    #[arg(long)]
    pub bucket_count: Option<usize>,
    /// IANA zone name for DST-correct local-hour aggregation.
    #[arg(long)]
    pub timezone: Option<String>,
    /// Minutes east of UTC fallback when --timezone is omitted (-1440 to 1440).
    #[arg(long, allow_hyphen_values = true)]
    pub timezone_offset_minutes: Option<i32>,
    /// Include stable facets; errors if any facet exceeds 1000 values.
    #[arg(long)]
    pub include_facets: bool,
}

#[derive(Debug, Args)]
pub struct GatewayUsageCallsArgs {
    #[arg(long)]
    pub since_unix: Option<i64>,
    #[arg(long)]
    pub until_unix: Option<i64>,
    #[arg(long)]
    pub upstream: Option<String>,
    /// Restrict to one qualified upstream::tool target.
    #[arg(long)]
    pub tool: Option<String>,
    /// Restrict to one capability family.
    #[arg(long)]
    pub capability: Option<String>,
    /// Restrict to one operation name.
    #[arg(long)]
    pub operation: Option<String>,
    /// Restrict by OAuth subject scoping (`true` or `false`).
    #[arg(long)]
    pub subject_scoped: Option<bool>,
    /// Restrict to one actor subject.
    #[arg(long)]
    pub actor: Option<String>,
    /// Restrict to one outcome; failed matches every non-ok outcome.
    #[arg(long)]
    pub outcome: Option<String>,
    /// Case-insensitive search across target, operation, actor, and outcome.
    #[arg(long)]
    pub search: Option<String>,
    #[arg(long)]
    pub limit: Option<usize>,
    /// Continue from the opaque cursor returned by the previous page.
    #[arg(long, conflicts_with = "offset")]
    pub cursor: Option<String>,
    /// Include the exact number of rows matching the filters.
    #[arg(long)]
    pub include_total: bool,
    /// Deprecated deep-offset pagination; use `--cursor` instead.
    #[arg(long)]
    pub offset: Option<usize>,
}

#[derive(Debug, Args)]
pub struct GatewayEnrichArgs {
    #[command(subcommand)]
    pub command: Option<GatewayEnrichCommand>,
    #[arg(long = "upstream")]
    pub upstreams: Vec<String>,
    #[arg(long)]
    pub all: bool,
    #[arg(long, default_value = "deterministic", value_parser = ["deterministic", "claude", "codex"])]
    pub provider: String,
    #[arg(long)]
    pub max_upstreams: Option<usize>,
    #[arg(long)]
    pub timeout_ms: Option<u64>,
    /// Skip confirmation for provider-backed preview runs.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

#[derive(Debug, Subcommand)]
pub enum GatewayEnrichCommand {
    Apply(GatewayEnrichApplyArgs),
}

#[derive(Debug, Args)]
pub struct GatewayEnrichApplyArgs {
    #[arg(long)]
    pub upstream: String,
    #[arg(long)]
    pub hint: String,
    #[arg(long, alias = "suggestion-hash")]
    pub metadata_hash: String,
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct GatewayCodeArgs {
    #[command(subcommand)]
    pub command: GatewayCodeCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayCodeCommand {
    /// Read gateway-wide Code Mode settings.
    Status,
    /// Enable the gateway codemode MCP surface.
    Enable,
    /// Disable the gateway codemode MCP surface.
    Disable,
    /// Manage the explicit Code Mode MCP App UI while keeping text execution available.
    Ui {
        #[command(subcommand)]
        command: GatewayCodeUiCommand,
    },
    /// Execute a sandboxed JavaScript snippet that calls the typed
    /// `codemode.<upstream>.<tool>` helpers (or `callTool` directly).
    Exec {
        #[arg(long, conflicts_with = "file")]
        code: Option<String>,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum GatewayCodeUiCommand {
    /// Read whether the explicit Code Mode MCP App UI is enabled.
    Status,
    /// Enable the explicit Code Mode MCP App UI.
    Enable,
    /// Disable the explicit Code Mode MCP App UI without disabling Code Mode.
    Disable,
}

#[derive(Debug, Args)]
pub struct GatewayPendingArgs {
    #[command(subcommand)]
    pub command: GatewayPendingCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayPendingCommand {
    /// List discovered servers waiting for approval
    List,
    /// Approve a pending server and add it to the gateway (disabled by default)
    Approve(GatewayPendingNameArgs),
    /// Reject a pending server and tombstone it so it never re-appears
    Reject(GatewayPendingNameArgs),
}

#[derive(Debug, Args)]
pub struct GatewayPendingNameArgs {
    pub name: String,
    /// Skip the destructive-action confirmation prompt.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be done without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct GatewayDiscoverArgs {
    /// Limit scan to specific client kinds (comma-separated: cursor,claude-code,vscode,...)
    #[arg(long, value_delimiter = ',')]
    pub clients: Vec<String>,
    /// Also show servers already present in the gateway config
    #[arg(long, default_value_t = false)]
    pub include_existing: bool,
}

#[derive(Debug, Args)]
pub struct GatewayImportArgs {
    /// Import every discovered server not already in the gateway config
    #[arg(long, default_value_t = false)]
    pub all: bool,
    /// Specific server names to import (space-separated)
    #[arg(long = "name")]
    pub names: Vec<String>,
    /// Limit discovery to specific client kinds (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub clients: Vec<String>,
    /// Skip confirmation for the destructive config import.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct GatewayGetArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct GatewayTestArgs {
    /// Name of a configured gateway to test (omit to test with inline --url/--command).
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct GatewayAddArgs {
    /// Unique name for the gateway upstream.
    #[arg(long)]
    pub name: String,
    /// HTTP(S) URL for a remote MCP server (mutually exclusive with --command).
    #[arg(long)]
    pub url: Option<String>,
    /// Stdio command to launch for a local MCP server (mutually exclusive with --url).
    #[arg(long)]
    pub command: Option<String>,
    /// Additional arguments passed to the stdio command (repeat for multiple).
    #[arg(long = "arg")]
    pub args: Vec<String>,
    /// Environment variable name whose value is used as the upstream bearer token.
    #[arg(long)]
    pub bearer_token_env: Option<String>,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub proxy_resources: bool,
    /// Aggregate this upstream's Agent Skills (SEP-2640) through the gateway.
    ///
    /// Defaults to false, unlike the other proxy flags: an upstream's skills
    /// carry instructions an agent will act on, so aggregating them is a
    /// deliberate trust decision. Without this flag there is no way to turn it
    /// on from the CLI at all.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    pub proxy_skills: bool,
    /// Initial skill-name exposure allowlist. Repeat for multiple patterns.
    #[arg(long = "expose-skill")]
    pub expose_skills: Vec<String>,
}

#[derive(Debug, Args)]
pub struct GatewayUpdateArgs {
    /// Name of the gateway upstream to update.
    pub name: String,
    /// Rename the gateway upstream to this new name.
    #[arg(long)]
    pub new_name: Option<String>,
    /// New HTTP(S) URL for a remote MCP server.
    #[arg(long, conflicts_with = "command")]
    pub url: Option<String>,
    /// Clear the HTTP(S) URL from this gateway.
    #[arg(long, conflicts_with = "url")]
    pub clear_url: bool,
    /// New stdio command for a local MCP server.
    #[arg(long, conflicts_with = "url")]
    pub command: Option<String>,
    /// Clear the stdio command from this gateway.
    #[arg(long, conflicts_with = "command")]
    pub clear_command: bool,
    /// Replace all command arguments with these values (repeat for multiple).
    #[arg(long = "arg")]
    pub args: Vec<String>,
    /// Environment variable name whose value is used as the upstream bearer token.
    #[arg(long)]
    pub bearer_token_env: Option<String>,
    /// Clear the upstream bearer token environment variable name.
    #[arg(long, conflicts_with = "bearer_token_env")]
    pub clear_bearer_token_env: bool,
    #[arg(long)]
    pub proxy_resources: Option<bool>,
    /// Turn Agent Skills aggregation on or off for this upstream.
    #[arg(long)]
    pub proxy_skills: Option<bool>,
    /// Replace the skill-name exposure allowlist. Repeat for multiple patterns.
    #[arg(long = "expose-skill", conflicts_with = "clear_expose_skills")]
    pub expose_skills: Vec<String>,
    /// Clear the skill-name allowlist and expose all validated Skills.
    #[arg(long, conflicts_with = "expose_skills")]
    pub clear_expose_skills: bool,
}

#[derive(Debug, Args)]
pub struct GatewayRemoveArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct GatewayQuarantineArgs {
    #[command(subcommand)]
    pub command: GatewayQuarantineCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayQuarantineCommand {
    /// List Lab-backed virtual servers quarantined during config migration.
    List,
    /// Restore a quarantined Lab-backed virtual server into the active gateway list.
    Restore(GatewayQuarantineRestoreArgs),
}

#[derive(Debug, Args)]
pub struct GatewayQuarantineRestoreArgs {
    pub id: String,
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteArgs {
    #[command(subcommand)]
    pub command: GatewayProtectedRouteCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayProtectedRouteCommand {
    /// List Gateway-managed public MCP routes protected by Lab OAuth.
    List,
    /// Get one Gateway-managed protected MCP route.
    Get(GatewayProtectedRouteNameArgs),
    /// Add a Gateway-managed protected MCP route.
    Add(GatewayProtectedRouteUpsertArgs),
    /// Replace a Gateway-managed protected MCP route.
    Update(GatewayProtectedRouteUpdateArgs),
    /// Remove a Gateway-managed protected MCP route.
    Remove(GatewayProtectedRouteRemoveArgs),
    /// Validate a proposed protected MCP route without saving it.
    Test(GatewayProtectedRouteUpsertArgs),
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteNameArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteRemoveArgs {
    pub name: String,
    /// Persist a gateway-subset removal for the next Labby restart.
    #[arg(long)]
    pub stage_for_restart: bool,
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteUpdateArgs {
    pub name: String,
    #[arg(long)]
    pub new_name: Option<String>,
    #[arg(long)]
    pub enabled: Option<bool>,
    #[arg(long)]
    pub public_host: String,
    #[arg(long)]
    pub public_path: String,
    #[arg(long)]
    pub upstream: Option<String>,
    #[arg(long)]
    pub backend_url: Option<String>,
    #[arg(long, hide = true)]
    pub backend_mcp_path: Option<String>,
    #[arg(long = "scope")]
    pub scopes: Vec<String>,
    #[arg(long)]
    pub health_path: Option<String>,
    /// Expose a scoped Lab gateway MCP surface instead of proxying one backend.
    #[arg(long)]
    pub gateway_subset: bool,
    /// Reuse a named Loadout for this gateway subset. Cannot be combined with inline target fields.
    #[arg(long, conflicts_with_all = ["target_upstream", "target_service", "expose_code_mode"])]
    pub loadout: Option<String>,
    /// Upstream names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_upstream: Vec<String>,
    /// Built-in Lab service names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_service: Vec<String>,
    /// Expose codemode on this gateway subset.
    #[arg(long)]
    pub expose_code_mode: bool,
    /// Persist this route change for the next Labby restart instead of attempting a hot route mutation.
    #[arg(long)]
    pub stage_for_restart: bool,
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteUpsertArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long, default_value_t = true)]
    pub enabled: bool,
    #[arg(long)]
    pub public_host: String,
    #[arg(long)]
    pub public_path: String,
    #[arg(long)]
    pub upstream: Option<String>,
    #[arg(long)]
    pub backend_url: Option<String>,
    #[arg(long, hide = true)]
    pub backend_mcp_path: Option<String>,
    #[arg(long = "scope")]
    pub scopes: Vec<String>,
    #[arg(long)]
    pub health_path: Option<String>,
    /// Expose a scoped Lab gateway MCP surface instead of proxying one backend.
    #[arg(long)]
    pub gateway_subset: bool,
    /// Reuse a named Loadout for this gateway subset. Cannot be combined with inline target fields.
    #[arg(long, conflicts_with_all = ["target_upstream", "target_service", "expose_code_mode"])]
    pub loadout: Option<String>,
    /// Upstream names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_upstream: Vec<String>,
    /// Built-in Lab service names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_service: Vec<String>,
    /// Expose codemode on this gateway subset.
    #[arg(long)]
    pub expose_code_mode: bool,
    /// Persist this route for the next Labby restart instead of attempting a hot route mount.
    #[arg(long)]
    pub stage_for_restart: bool,
}

#[derive(Debug, Args)]
pub struct GatewayMcpArgs {
    #[command(subcommand)]
    pub command: GatewayMcpCommand,
}

#[derive(Debug, Args)]
pub struct GatewayClientsArgs {
    #[command(subcommand)]
    pub command: GatewayClientsCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayClientsCommand {
    /// List inbound MCP clients/sessions currently connected to this gateway.
    /// Best-effort — reflects the most recently observed connect events, not
    /// a strict live liveness view.
    List,
}

#[derive(Debug, Subcommand)]
pub enum GatewayMcpCommand {
    /// Manage upstream MCP server OAuth credentials.
    Auth(GatewayMcpAuthArgs),
    /// List upstream MCP runtime state, discovery counts, and likely stale process counts.
    List,
    /// Enable an upstream MCP server so new sessions discover and proxy it again.
    Enable(GatewayMcpLifecycleArgs),
    /// Disable an upstream MCP server and optionally clean up running processes.
    Disable(GatewayMcpLifecycleArgs),
    /// Replace one enabled upstream MCP connection and clean up stale runtime processes.
    Restart(GatewayMcpRestartArgs),
    /// Kill or preview running processes associated with one upstream MCP server.
    Cleanup(GatewayMcpCleanupArgs),
}

#[derive(Debug, Args)]
pub struct GatewayMcpAuthArgs {
    #[command(subcommand)]
    pub command: GatewayMcpAuthCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayMcpAuthCommand {
    /// Start the upstream OAuth flow and print the browser authorization URL.
    Start(GatewayOauthUpstreamArgs),
    /// Start the upstream OAuth flow and open the authorization URL in a browser.
    Open(GatewayOauthUpstreamArgs),
    /// Read upstream OAuth status for the shared gateway credential.
    Status(GatewayOauthUpstreamArgs),
    /// Clear stored dedicated upstream OAuth credentials.
    Clear(GatewayOauthUpstreamArgs),
    /// Revoke the central Google provider credential and all dependent Labby grants.
    RevokeGoogle(GatewayOauthRevokeArgs),
}

#[derive(Debug, Args)]
pub struct GatewayOauthUpstreamArgs {
    pub name: String,
    #[arg(long, default_value_t = false)]
    pub open: bool,
    #[arg(long, default_value_t = false)]
    pub wait: bool,
    #[arg(long, default_value_t = 120)]
    pub wait_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub struct GatewayOauthRevokeArgs {
    pub name: String,
    /// Confirm revocation of a credential shared by Google MCP servers and inbound grants.
    #[arg(long, default_value_t = false)]
    pub confirm: bool,
}

#[derive(Debug, Args)]
pub struct GatewayMcpLifecycleArgs {
    pub name: String,
    #[arg(long, default_value_t = false)]
    pub cleanup: bool,
    #[arg(long, default_value_t = false)]
    pub aggressive: bool,
}

#[derive(Debug, Args)]
pub struct GatewayMcpCleanupArgs {
    pub name: String,
    #[arg(long, default_value_t = false)]
    pub aggressive: bool,
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct GatewayMcpRestartArgs {
    /// Name of the enabled upstream MCP server to reconnect.
    pub name: String,
    /// Use broader host-wide process matching when cleaning up the old runtime.
    #[arg(long, default_value_t = false)]
    pub aggressive: bool,
}
