//! Target registry, replay engine, replay-driven normalization, and the
//! `verify` command line.
//!
//! Replay is the common denominator of the whole toolkit: every counterexample
//! origin — model checker, fuzzer, or production incident — reduces to a
//! scenario file that this engine executes. It is pure, deterministic, and
//! needs no external toolchain, which is why it ships before any backend.
//!
//! This crate depends on **no backend crate**. The `Backend` trait lives in
//! `verify-core`, and an adopting project registers the implementations it
//! wants. That inversion is what lets a project use Stateright without
//! installing a Java toolchain.
//!
//! # Building a `verify` binary
//!
//! A standalone binary cannot know a project's targets, so the shipped `verify`
//! bin has an empty registry — enough for `catalog validate`, which needs no
//! target. An adopting project writes a one-line binary of its own:
//!
//! ```no_run
//! # use verify_runner::{TargetRegistry, cli};
//! fn main() -> std::process::ExitCode {
//!     let registry = TargetRegistry::new();
//!     // .with(TargetKey::new("labby", "gateway"), Box::new(GatewayModel))
//!     cli::run_from_env(&registry)
//! }
//! ```

pub mod cli;
pub mod dyn_target;
pub mod minimize;
pub mod registry;
pub mod replay;

pub use dyn_target::{DynTarget, ErasedState};
pub use minimize::{DeterminismReport, check_determinism, minimize, normalize};
pub use registry::{TargetKey, TargetRegistry};
pub use replay::replay;
