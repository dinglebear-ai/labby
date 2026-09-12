//! The shipped `verify` binary: an empty target registry.
//!
//! This is enough for `catalog validate`, which needs no target. Replaying a
//! project's scenarios needs that project's targets, which a standalone binary
//! cannot know — an adopting project links `verify-runner` into a binary of its
//! own and registers them. See the crate docs.

use std::process::ExitCode;

use verify_runner::{TargetRegistry, cli};

fn main() -> ExitCode {
    cli::run_from_env(&TargetRegistry::new())
}
