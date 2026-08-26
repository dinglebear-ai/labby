use std::future::Future;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::cli::helpers::run_action_command;
use crate::config::LabConfig;
use crate::output::OutputFormat;

async fn dispatch_at_cli_boundary(
    registry: &crate::skills::facade::SkillRegistryContext,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, crate::dispatch::error::ToolError> {
    crate::dispatch::skills::dispatch_with_context(registry, action, params).await
}

#[derive(Debug, Args)]
pub struct SkillsArgs {
    /// Canonical Access project used to authorize Artifact-backed Skills.
    #[arg(long, global = true)]
    pub project_id: String,
    #[command(subcommand)]
    pub command: SkillsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCommand {
    /// List caller-visible Agent Skills without loading their bodies.
    List(SkillsListArgs),
    /// Search Agent Skills by name, description, and metadata.
    Search(SkillsSearchArgs),
    /// Show one skill entry by published URI.
    Get(SkillsUriArgs),
    /// Read one manifest-bound skill file by published URI.
    Read(SkillsUriArgs),
}

#[derive(Debug, Args)]
pub struct SkillsListArgs {
    /// Restrict results to one visible origin label.
    #[arg(long)]
    pub origin: Option<String>,
    /// Maximum number of entries to return.
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Args)]
pub struct SkillsSearchArgs {
    /// Metadata search query.
    pub query: String,
    /// Restrict results to one visible origin label.
    #[arg(long)]
    pub origin: Option<String>,
    /// Maximum number of matches to return.
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Args)]
pub struct SkillsUriArgs {
    /// Published skill or skill-file URI.
    pub uri: String,
}

pub fn run<'a>(
    args: SkillsArgs,
    format: OutputFormat,
    config: &'a LabConfig,
) -> std::pin::Pin<Box<dyn Future<Output = Result<ExitCode>> + Send + 'a>> {
    Box::pin(run_inner(args, format, config))
}

async fn run_inner(args: SkillsArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    let project_id = args.project_id;
    let (action, params) = match args.command {
        SkillsCommand::List(args) => (
            "skills.list".to_string(),
            json!({ "origin": args.origin, "limit": args.limit }),
        ),
        SkillsCommand::Search(args) => (
            "skills.search".to_string(),
            json!({
                "query": args.query,
                "origin": args.origin,
                "limit": args.limit,
            }),
        ),
        SkillsCommand::Get(args) => ("skills.get".to_string(), json!({ "uri": args.uri })),
        SkillsCommand::Read(args) => ("skills.read".to_string(), json!({ "uri": args.uri })),
    };

    #[cfg(feature = "gateway")]
    {
        let manager = crate::cli::gateway::build_manager(config, true).await?;
        let access = if manager.code_mode_enabled().await {
            crate::skills::aggregate::ToolAccess::CodeModeOnly
        } else {
            crate::skills::aggregate::ToolAccess::Direct
        };
        let scope = crate::skills::facade::SkillCallerScope::root(
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT.to_string()),
            access,
        );
        return run_action_command("skills", action, params, format, move |action, params| {
            let manager = std::sync::Arc::clone(&manager);
            let scope = scope.clone();
            let project_id = project_id.clone();
            async move {
                let mut registry =
                    crate::skills::facade::SkillRegistryContext::with_manager(manager, scope);
                if let Some(access) = cli_artifact_access_snapshot(&project_id).await {
                    registry = registry.with_artifact_access(access);
                }
                dispatch_at_cli_boundary(&registry, &action, params).await
            }
        })
        .await;
    }

    #[cfg(not(feature = "gateway"))]
    {
        let _ = config;
        let access = cli_artifact_access_snapshot(&project_id).await;
        run_action_command("skills", action, params, format, move |action, params| {
            let access = access.clone();
            async move {
                let mut registry = crate::skills::facade::SkillRegistryContext::first_party_only();
                if let Some(access) = access {
                    registry = registry.with_artifact_access(access);
                }
                dispatch_at_cli_boundary(&registry, &action, params).await
            }
        })
        .await
    }
}

#[cfg(target_os = "linux")]
async fn cli_artifact_access_snapshot(
    project_id: &str,
) -> Option<crate::skills::facade::ArtifactAccessSnapshot> {
    let subject = format!(
        "unix-peer:uid={}:gid={}",
        nix::unistd::geteuid().as_raw(),
        nix::unistd::getegid().as_raw()
    );
    let identity = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::UnixPeer,
        subject,
    )
    .ok()?;
    let runtime =
        crate::access::AccessRuntime::initialize(crate::config::access_db_path().ok()?).await;
    cli_artifact_access_snapshot_for(&runtime, identity, project_id).await
}

async fn cli_artifact_access_snapshot_for(
    runtime: &crate::access::AccessRuntime,
    identity: labby_auth::VerifiedIdentity,
    project_id: &str,
) -> Option<crate::skills::facade::ArtifactAccessSnapshot> {
    let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
        identity,
        Vec::<String>::new(),
        crate::dispatch::skill_library::auth::SkillLibraryTransport {
            surface: crate::dispatch::skill_library::auth::SkillLibrarySurface::Cli,
            same_origin: false,
            csrf_verified: false,
            audience_bound: false,
            host_established_callback: false,
        },
    );
    let correlation =
        crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse("cli-skills-read")
            .ok()?;
    crate::dispatch::skill_library::auth::authorize_at_boundary(
        runtime,
        caller,
        project_id,
        crate::dispatch::skill_library::auth::SkillLibraryAction::List,
        &crate::dispatch::skill_library::audit::CanonicalArtifactId::parse("library").ok()?,
        crate::dispatch::skill_library::auth::SkillLibraryTarget::SharedActive,
        &correlation,
    )
    .await
    .ok()
    .map(|decision| decision.artifact_access_snapshot())
}

#[cfg(not(target_os = "linux"))]
async fn cli_artifact_access_snapshot(
    _project_id: &str,
) -> Option<crate::skills::facade::ArtifactAccessSnapshot> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Subcommand;
    use labby_auth::{Authenticator, VerifiedIdentity};

    #[test]
    fn cli_help_exposes_only_implemented_read_commands() {
        let command = SkillsCommand::augment_subcommands(clap::Command::new("skills"));
        let names = command
            .get_subcommands()
            .map(|subcommand| subcommand.get_name())
            .collect::<Vec<_>>();
        assert_eq!(names, ["list", "search", "get", "read"]);
        assert!(names.iter().all(|name| !name.contains("callback")));
    }

    #[tokio::test]
    async fn cli_live_access_snapshot_observes_role_changes_and_revocation_once_per_request() {
        use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput};
        use labby_runtime::artifacts::{
            LibraryActorId, LibraryOwnership, LibraryTenantId, SkillVisibility,
        };

        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().join("access.db");
        let identity = VerifiedIdentity::local_credential(
            Authenticator::UnixPeer,
            "unix-peer:uid=1000:gid=1000",
        )
        .unwrap();
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        drop(store);
        let runtime = AccessRuntime::initialize(path).await;
        let other = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("bootstrap-local").unwrap(),
            LibraryActorId::from_canonical_projection("other").unwrap(),
        );

        let owner =
            cli_artifact_access_snapshot_for(&runtime, identity.clone(), "bootstrap-default")
                .await
                .unwrap();
        let store = runtime.store().await.unwrap();
        assert_eq!(store.skill_library_authorization_count_for_test(), 1);
        assert!(owner.permits(&other, SkillVisibility::Private));

        store.execute_test_statement(
            "UPDATE project_memberships SET role='member' WHERE membership_id='bootstrap-owner-membership'",
        ).await.unwrap();
        let member =
            cli_artifact_access_snapshot_for(&runtime, identity.clone(), "bootstrap-default")
                .await
                .unwrap();
        assert_eq!(store.skill_library_authorization_count_for_test(), 2);
        assert!(!member.permits(&other, SkillVisibility::Private));
        assert!(member.permits(&other, SkillVisibility::Tenant));

        store.execute_test_statement(
            "UPDATE project_memberships SET status='suspended' WHERE membership_id='bootstrap-owner-membership'",
        ).await.unwrap();
        assert!(
            cli_artifact_access_snapshot_for(&runtime, identity, "bootstrap-default")
                .await
                .is_none()
        );
        assert_eq!(store.skill_library_authorization_count_for_test(), 3);
    }

    #[tokio::test]
    async fn cli_boundary_keeps_a_captured_generation_during_refresh() {
        use crate::skills::facade::SkillRegistryContext;
        use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cli-race");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: cli-race\ndescription: old\n---\nold\n",
        )
        .unwrap();
        let manager =
            FirstPartyGenerationManager::new(temp.path().into(), GenerationLimits::default());
        let old = SkillRegistryContext::from_generation(manager.generation());
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: cli-race\ndescription: new\n---\nnew\n",
        )
        .unwrap();
        manager.refresh(None).unwrap();
        let old_value = dispatch_at_cli_boundary(
            &old,
            "skills.read",
            serde_json::json!({"uri":"skill://labby/cli-race/SKILL.md"}),
        )
        .await
        .unwrap();
        let current = SkillRegistryContext::from_generation(manager.generation());
        let new_value = dispatch_at_cli_boundary(
            &current,
            "skills.read",
            serde_json::json!({"uri":"skill://labby/cli-race/SKILL.md"}),
        )
        .await
        .unwrap();
        assert!(old_value["text"].as_str().unwrap().contains("old"));
        assert!(new_value["text"].as_str().unwrap().contains("new"));
        assert_ne!(old_value["digest"], new_value["digest"]);
    }
}
