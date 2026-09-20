//! Local skill reads and daemon-backed source policy are separate operations.

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommand,
}

#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    #[cfg(feature = "skills")]
    #[command(flatten)]
    Local(super::skills::SkillsCommand),
    /// Manage daemon-backed upstream skill trust and exposure.
    #[cfg(feature = "gateway")]
    Source(SourceArgs),
}

#[cfg(feature = "gateway")]
#[derive(Debug, Args)]
pub struct SourceArgs {
    #[command(subcommand)]
    pub command: SourceCommand,
}

#[cfg(feature = "gateway")]
#[derive(Debug, Subcommand)]
pub enum SourceCommand {
    /// Report upstream skill support, trust, validation, and exposure.
    List(super::gateway::GatewaySkillsListArgs),
    /// Trust an upstream's skill instructions and allow enumeration.
    Trust(super::gateway::GatewaySkillsTrustArgs),
    /// Stop trusting an upstream's skill instructions.
    Untrust(super::gateway::GatewaySkillsUpstreamArgs),
    /// Manage a trusted upstream's skill exposure allowlist.
    Exposure {
        #[command(subcommand)]
        command: ExposureCommand,
    },
}

#[cfg(feature = "gateway")]
#[derive(Debug, Subcommand)]
pub enum ExposureCommand {
    /// Replace the skill-name allowlist with explicit patterns.
    Set(super::gateway::GatewaySkillsExposeArgs),
    /// Clear the allowlist, exposing all validated skills from the trusted upstream.
    Clear(super::gateway::GatewaySkillsUpstreamArgs),
}

impl SkillArgs {
    pub fn operation(self) -> super::Command {
        match self.command {
            #[cfg(feature = "skills")]
            SkillCommand::Local(command) => {
                super::Command::Skills(super::skills::SkillsArgs { command })
            }
            #[cfg(feature = "gateway")]
            SkillCommand::Source(args) => {
                use super::gateway::*;
                let command = match args.command {
                    SourceCommand::List(args) => GatewaySkillsCommand::List(args),
                    SourceCommand::Trust(args) => GatewaySkillsCommand::Trust(args),
                    SourceCommand::Untrust(args) => GatewaySkillsCommand::Untrust(args),
                    SourceCommand::Exposure {
                        command: ExposureCommand::Set(args),
                    } => GatewaySkillsCommand::Expose(args),
                    SourceCommand::Exposure {
                        command: ExposureCommand::Clear(args),
                    } => GatewaySkillsCommand::ExposeAll(args),
                };
                super::Command::Gateway(GatewayArgs {
                    command: GatewayCommand::Skills(GatewaySkillsArgs { command }),
                })
            }
        }
    }
}
