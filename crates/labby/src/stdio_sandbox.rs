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
        ABI, Access as _, AccessFs, CompatLevel, Compatible as _, Ruleset, RulesetAttr as _,
        RulesetCreatedAttr as _, RulesetStatus, path_beneath_rules,
    };
    use std::os::unix::process::CommandExt as _;

    pub(super) fn run(args: &[OsString]) -> anyhow::Result<ExitCode> {
        let (read_only, read_write, command) = parse(args)?;
        match std::env::var("LABBY_STDIO_SANDBOX_BACKEND").as_deref() {
            Ok("bubblewrap") => {
                eprintln!("stdio sandbox backend: bubblewrap");
                return Err(std::process::Command::new("/usr/bin/bwrap")
                    .args(bubblewrap_args(&read_only, &read_write, command))
                    .exec()
                    .into());
            }
            Ok("landlock") | Err(std::env::VarError::NotPresent) => {}
            _ => bail!("unsupported LABBY_STDIO_SANDBOX_BACKEND; choose landlock or bubblewrap"),
        }
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
                Some("--read-only" | "--read-write") => {
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
                _ => bail!("invalid stdio sandbox argument: {}", args[index].display()),
            }
        }
        bail!("missing -- before sandboxed command")
    }
}

#[cfg(target_os = "linux")]
fn bubblewrap_args(
    read_only: &[OsString],
    read_write: &[OsString],
    command: &[OsString],
) -> Vec<OsString> {
    // Retain the gateway-owned process group. A new session lets descendants
    // escape its cancellation guard; stdio uses pipes and exposes no terminal.
    let mut args = [
        "--die-with-parent",
        "--unshare-all",
        "--share-net",
        "--cap-drop",
        "ALL",
        "--chdir",
        "/",
    ]
    .map(OsString::from)
    .to_vec();
    for (flag, paths) in [("--ro-bind", read_only), ("--bind", read_write)] {
        for path in paths {
            // Ordinary bind mounts retain nodev; SSH must be able to open its
            // explicitly granted null device without exposing the /dev tree.
            let flag = if flag == "--bind" && path == "/dev/null" {
                "--dev-bind"
            } else {
                flag
            };
            args.extend([OsString::from(flag), path.clone(), path.clone()]);
        }
    }
    if let Ok(cwd) = std::env::current_dir()
        && read_only
            .iter()
            .chain(read_write)
            .any(|path| cwd.starts_with(std::path::Path::new(path)))
    {
        args.extend([OsString::from("--chdir"), cwd.into_os_string()]);
    }
    args.push(OsString::from("--"));
    args.extend_from_slice(command);
    args
}

#[cfg(all(test, target_os = "linux"))]
mod bubblewrap_tests {
    use super::*;

    #[test]
    fn bubblewrap_grants_null_as_a_device_without_exposing_other_devices() {
        let args = bubblewrap_args(
            &[],
            &[OsString::from("/dev/null")],
            &[OsString::from("/usr/bin/true")],
        );
        assert!(
            args.windows(3)
                .any(|p| p == ["--dev-bind", "/dev/null", "/dev/null"].map(OsString::from))
        );
        assert!(!args.iter().any(|arg| arg == "/dev"));
    }

    #[test]
    fn bubblewrap_preserves_explicit_filesystem_grants_and_isolates_namespaces() {
        let args = bubblewrap_args(
            &[OsString::from("/usr")],
            &[OsString::from("/var/lib/upstream")],
            &[OsString::from("/usr/bin/ssh"), OsString::from("-T")],
        );
        assert!(
            args.starts_with(
                &[
                    "--die-with-parent",
                    "--unshare-all",
                    "--share-net",
                    "--cap-drop",
                    "ALL",
                    "--chdir",
                    "/",
                ]
                .map(OsString::from)
            )
        );
        assert!(
            args.windows(3)
                .any(|p| p == ["--ro-bind", "/usr", "/usr"].map(OsString::from))
        );
        assert!(
            args.windows(3)
                .any(|p| p
                    == ["--bind", "/var/lib/upstream", "/var/lib/upstream"].map(OsString::from))
        );
        assert!(args.ends_with(&["--", "/usr/bin/ssh", "-T"].map(OsString::from)));
        for forbidden in ["/home", "/proc", "/tmp", "/run/user"] {
            assert!(!args.iter().any(|arg| arg == forbidden));
        }
    }
}
