//! The `verify` command line.
//!
//! Exit codes are a contract, not an afterthought:
//!
//! | Code | Meaning |
//! | --- | --- |
//! | 0 | every gating scenario matched its `expect` |
//! | 1 | a mismatch — the real failure |
//! | 2 | no target registered for a scenario's `(project, model)` |
//! | 3 | malformed scenario or catalog |
//!
//! 2 is separate from 1 on purpose: "nobody registered the target" must never
//! be readable as "the invariant holds".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use verify_core::{Catalog, Kind};
use verify_report::replay::{Outcome, Summary};
use verify_report::text::{render_summary, render_text};
use verify_scenario::Scenario;

use crate::registry::TargetRegistry;
use crate::replay::replay;

/// Exit code for a scenario that did not match its expectation.
pub const EXIT_MISMATCH: u8 = 1;
/// Exit code for a scenario naming an unregistered target.
pub const EXIT_NO_TARGET: u8 = 2;
/// Exit code for input that could not be read at all.
pub const EXIT_MALFORMED: u8 = 3;

#[derive(Debug, Parser)]
#[command(name = "verify", about = "Replay verification scenarios", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Replay one or more scenario files.
    Replay {
        /// Scenario files to replay.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// How to judge the invariant when no catalog is supplied.
        #[arg(long, value_enum, default_value = "safety")]
        kind: KindArg,
        /// Catalog to take each invariant's kind from.
        #[arg(long)]
        catalog: Option<PathBuf>,
    },
    /// Parse and validate an invariant catalog.
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
    /// List the targets this binary can replay against.
    Targets,
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// Check a catalog against every structural rule.
    Validate { path: PathBuf },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum KindArg {
    Safety,
    Liveness,
    Security,
    Refinement,
}

impl From<KindArg> for Kind {
    fn from(value: KindArg) -> Self {
        match value {
            KindArg::Safety => Self::Safety,
            KindArg::Liveness => Self::Liveness,
            KindArg::Security => Self::Security,
            KindArg::Refinement => Self::Refinement,
        }
    }
}

/// Parse arguments from the environment and run.
pub fn run_from_env(registry: &TargetRegistry) -> ExitCode {
    run(&Cli::parse(), registry)
}

/// Run an already-parsed command. Separated so tests can drive the CLI without
/// a process boundary.
pub fn run(cli: &Cli, registry: &TargetRegistry) -> ExitCode {
    match &cli.command {
        Command::Replay {
            paths,
            kind,
            catalog,
        } => run_replay(paths, (*kind).into(), catalog.as_deref(), registry),
        Command::Catalog {
            command: CatalogCommand::Validate { path },
        } => run_catalog_validate(path),
        Command::Targets => {
            if registry.is_empty() {
                println!(
                    "no targets registered; link verify-runner into a binary that registers yours"
                );
            }
            for key in registry.keys() {
                println!("{} / {}", key.project, key.model);
            }
            ExitCode::SUCCESS
        }
    }
}

fn run_replay(
    paths: &[PathBuf],
    default_kind: Kind,
    catalog: Option<&Path>,
    registry: &TargetRegistry,
) -> ExitCode {
    let loaded_catalog = match catalog {
        Some(path) => match load_catalog(path) {
            Ok(catalog) => Some(catalog),
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(EXIT_MALFORMED);
            }
        },
        None => None,
    };

    let mut summary = Summary::default();
    let mut worst: Option<u8> = None;

    for path in paths {
        let label = path.display().to_string();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("MALFORMED   {label}\n            {error}");
                worst = Some(worst.map_or(EXIT_MALFORMED, |code| code.max(EXIT_MALFORMED)));
                summary.malformed += 1;
                summary.total += 1;
                summary.failing += 1;
                continue;
            }
        };
        let scenario = match Scenario::parse(&text) {
            Ok(scenario) => scenario,
            Err(error) => {
                eprintln!("MALFORMED   {label}\n            {error}");
                worst = Some(worst.map_or(EXIT_MALFORMED, |code| code.max(EXIT_MALFORMED)));
                summary.malformed += 1;
                summary.total += 1;
                summary.failing += 1;
                continue;
            }
        };
        let kind = match scenario_kind(&scenario, loaded_catalog.as_ref(), default_kind) {
            Ok(kind) => kind,
            Err(reason) => {
                eprintln!("MALFORMED   {label}\n            {reason}");
                worst = Some(EXIT_MALFORMED);
                summary.malformed += 1;
                summary.total += 1;
                summary.failing += 1;
                continue;
            }
        };
        let report = replay(&scenario, kind, registry, &label);
        println!("{}", render_text(&report));
        if report.failed() {
            let code = match report.outcome {
                Outcome::NoTarget { .. } => EXIT_NO_TARGET,
                Outcome::Malformed { .. } => EXIT_MALFORMED,
                _ => EXIT_MISMATCH,
            };
            worst = Some(worst.map_or(code, |current| current.max(code)));
        }
        summary.accumulate(&report);
    }

    println!("{}", render_summary(&summary));
    worst.map_or(ExitCode::SUCCESS, ExitCode::from)
}

fn load_catalog(path: &Path) -> Result<Catalog, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read catalog {}: {error}", path.display()))?;
    let catalog = Catalog::parse(&text)
        .map_err(|error| format!("cannot parse catalog {}: {error}", path.display()))?;
    catalog.validate_structure().map_err(|errors| {
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    Ok(catalog)
}

fn scenario_kind(
    scenario: &Scenario,
    catalog: Option<&Catalog>,
    default: Kind,
) -> Result<Kind, String> {
    let Some(catalog) = catalog else {
        return Ok(default);
    };
    if catalog.project != scenario.project {
        return Err(format!(
            "catalog project {} does not match scenario project {}",
            catalog.project, scenario.project
        ));
    }
    let invariant = catalog
        .invariants
        .iter()
        .find(|entry| entry.id == scenario.invariant)
        .ok_or_else(|| {
            format!(
                "invariant {} is absent from the supplied catalog",
                scenario.invariant
            )
        })?;
    if invariant.model != scenario.model {
        return Err(format!(
            "invariant {} belongs to model {}, not {}",
            invariant.id, invariant.model, scenario.model
        ));
    }
    Ok(invariant.kind)
}

fn run_catalog_validate(path: &Path) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read {}: {error}", path.display());
            return ExitCode::from(EXIT_MALFORMED);
        }
    };
    let catalog = match Catalog::parse(&text) {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(EXIT_MALFORMED);
        }
    };
    // No backends are registered in the shipped binary, so every binding reads
    // as unknown. That is the correct answer rather than a reason to skip the
    // check: a catalog binding backends this binary cannot see is not validated.
    match catalog.validate(&BTreeMap::new()) {
        Ok(()) => {
            let uncovered = catalog.uncovered();
            println!(
                "catalog ok: {} invariants, {} uncovered",
                catalog.invariants.len(),
                uncovered.len()
            );
            for id in uncovered {
                println!("  uncovered {id}");
            }
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in &errors {
                eprintln!("{error}");
            }
            ExitCode::from(EXIT_MALFORMED)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Catalog {
        Catalog::parse(
            r#"schema = 1
project = "fixture"
namespace = "FIXTURE"
[[invariant]]
id = "FIXTURE-REQ-001"
title = "Request"
kind = "liveness"
severity = "high"
model = "request"
"#,
        )
        .expect("catalog")
    }
    fn scenario() -> Scenario {
        Scenario::parse(r#"{"schema":1,"project":"fixture","model":"request","invariant":"FIXTURE-REQ-001","origin":{"kind":"manual"},"steps":[],"expect":"invariant_holds"}"#).expect("scenario")
    }
    #[test]
    fn supplied_catalog_never_falls_back_for_an_unknown_identity() {
        let catalog = catalog();
        let original = scenario();
        assert_eq!(
            scenario_kind(&original, Some(&catalog), Kind::Safety),
            Ok(Kind::Liveness)
        );
        let mut changed = original.clone();
        changed.invariant = verify_core::InvariantId::parse("FIXTURE-REQ-002").expect("id");
        assert!(scenario_kind(&changed, Some(&catalog), Kind::Safety).is_err());
        changed = original.clone();
        changed.project = "other".into();
        assert!(scenario_kind(&changed, Some(&catalog), Kind::Safety).is_err());
        changed = original;
        changed.model = "other".into();
        assert!(scenario_kind(&changed, Some(&catalog), Kind::Safety).is_err());
    }
    #[test]
    fn structural_validation_rejects_future_schemas_and_duplicate_ids() {
        let mut catalog = catalog();
        catalog.schema = 2;
        assert!(catalog.validate_structure().is_err());
        catalog.schema = 1;
        catalog.invariants.push(catalog.invariants[0].clone());
        assert!(catalog.validate_structure().is_err());
    }
}
