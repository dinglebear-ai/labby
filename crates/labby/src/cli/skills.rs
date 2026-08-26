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
            async move {
                let registry =
                    crate::skills::facade::SkillRegistryContext::with_manager(manager, scope);
                dispatch_at_cli_boundary(&registry, &action, params).await
            }
        })
        .await;
    }

    #[cfg(not(feature = "gateway"))]
    {
        let _ = config;
        run_action_command(
            "skills",
            action,
            params,
            format,
            |action, params| async move { crate::dispatch::skills::dispatch(&action, params).await },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
