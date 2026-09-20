use std::{ffi::OsString, io::Write, path::Path};

use crate::{ReplayLimits, TargetRegistry, corpus::read_scenario};

/// Shared CLI adapter for the adopting project's registry-populated binary.
/// Arguments omit argv[0]. Exit 0: no active mismatch; 1: active mismatch;
/// 2: usage, unreadable/invalid envelope, or output failure. One JSON report per
/// readable scenario, including non-gating errors/quarantined evidence.
pub fn run_cli(
    registry: &TargetRegistry<'_>,
    args: impl IntoIterator<Item = OsString>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let mut args = args.into_iter();
    let command = args.next();
    if command.as_deref() == Some(std::ffi::OsStr::new("--help")) && args.next().is_none() {
        return if writeln!(output, "verify replay <scenario.json>...\nTargets must be registered by the adopting binary; scenario files never load code.").is_ok() { 0 } else { 2 };
    }
    if command.as_deref() != Some(std::ffi::OsStr::new("replay")) {
        let _ = writeln!(errors, "usage: verify replay <scenario.json>...");
        return 2;
    }
    let paths: Vec<_> = args.collect();
    if paths.is_empty() {
        let _ = writeln!(errors, "verify replay requires at least one scenario file");
        return 2;
    }
    let mut exit = 0;
    for path in paths {
        let scenario = match read_scenario(Path::new(&path)) {
            Ok(scenario) => scenario,
            Err(error) => {
                let _ = writeln!(
                    errors,
                    "cannot load {}: {error}",
                    Path::new(&path).display()
                );
                exit = 2;
                continue;
            }
        };
        let report = registry.replay(&scenario, &ReplayLimits::default());
        if report.gate_failure {
            exit = exit.max(1);
        }
        if serde_json::to_writer(&mut *output, &report).is_err() || writeln!(output).is_err() {
            return 2;
        }
    }
    exit
}
