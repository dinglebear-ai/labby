//! Portable scenario envelope, fingerprinting, and syntactic normalization.
//!
//! A scenario is a replayable trace: an initial state plus an ordered list of
//! steps. The envelope is project-agnostic; `initial` and `steps` are opaque
//! here and interpreted only by the adopting project's `ScenarioTarget`.
//!
//! Only the **syntactic** normalization passes live in this crate — canonical
//! identifier renaming and commutativity reordering, both pure functions over
//! the envelope. The replay-driven passes (prefix minimization, the determinism
//! check) must execute a scenario to learn whether a step mattered, and replay
//! lives in `verify-runner`, which already depends on this crate. Putting them
//! here would be a dependency cycle.

pub mod envelope;
pub mod fingerprint;
pub mod normalize;

pub use envelope::{
    Expect, Origin, OriginKind, SCENARIO_SCHEMA, Scenario, ScenarioLoadError, ScenarioStatus,
};
pub use fingerprint::{FINGERPRINT_PREFIX, fingerprint};
pub use normalize::{Commutes, normalize_syntactic};
