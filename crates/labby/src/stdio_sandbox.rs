use std::ffi::OsString;
use std::process::ExitCode;

const INTERNAL_COMMAND: &str = "__stdio-sandbox";

pub(crate) fn maybe_run(argv: &[OsString]) -> Option<ExitCode> {
    (argv.get(1).and_then(|arg| arg.to_str()) == Some(INTERNAL_COMMAND)).then(|| run(&argv[2..]))
}

#[cfg(not(target_os = "linux"))]
fn run(_args: &[OsString]) -> ExitCode {
    eprintln!("stdio sandbox is unavailable on this platform");
    ExitCode::from(78)
}

#[cfg(target_os = "linux")]
fn run(args: &[OsString]) -> ExitCode {
    match linux::run(args) {
        Ok(never) => never,
        Err(error) => {
            eprintln!("stdio sandbox refused to launch: {error:#}");
            ExitCode::from(78)
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::{anyhow, bail};
    use landlock::{
        ABI, Access as _, AccessFs, CompatLevel, Ruleset, RulesetAttr as _,
        RulesetCreatedAttr as _, RulesetStatus, path_beneath_rules,
    };
    use std::os::unix::process::CommandExt as _;

    pub(super) fn run(args: &[OsString]) -> anyhow::Result<ExitCode> {
        let (read_only, read_write, command) = parse(args)?;
        let abi = ABI::V3;
        let mut ruleset = Ruleset::default()
            .handle_access(AccessFs::from_all(abi))?
            .create()?;
        ruleset = ruleset.add_rules(path_beneath_rules(
            read_only.iter(),
            AccessFs::from_read(abi),
        ))?;
        ruleset = ruleset.add_rules(path_beneath_rules(
            read_write.iter(),
            AccessFs::from_all(abi),
        ))?;
        let status = ruleset
            .set_compatibility(CompatLevel::HardRequirement)
            .restrict_self()?;
        if status.ruleset == RulesetStatus::NotEnforced {
            bail!("kernel did not enforce the required Landlock ruleset");
        }

        let (program, command_args) = command
            .split_first()
            .ok_or_else(|| anyhow!("missing command after --"))?;
        Err(std::process::Command::new(program)
            .args(command_args)
            .exec()
            .into())
    }

    fn parse(args: &[OsString]) -> anyhow::Result<(Vec<OsString>, Vec<OsString>, &[OsString])> {
        let mut read_only = Vec::new();
        let mut read_write = Vec::new();
        let mut index = 0;
        while index < args.len() {
            match args[index].to_str() {
                Some("--") => return Ok((read_only, read_write, &args[index + 1..])),
                Some("--read-only") | Some("--read-write") => {
                    let writable = args[index] == "--read-write";
                    let path = args
                        .get(index + 1)
                        .ok_or_else(|| anyhow!("missing sandbox path"))?;
                    if writable {
                        read_write.push(path.clone());
                    } else {
                        read_only.push(path.clone());
                    }
                    index += 2;
                }
                _ => bail!("invalid stdio sandbox argument: {:?}", args[index]),
            }
        }
        bail!("missing -- before sandboxed command")
    }
}
