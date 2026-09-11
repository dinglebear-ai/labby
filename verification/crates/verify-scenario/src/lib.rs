//! Portable scenario envelope, fingerprinting, and syntactic normalization.
//!
//! A scenario is a replayable trace: an initial state plus an ordered list of
//! steps. The envelope is project-agnostic; `initial` and `steps` are opaque
//! here and interpreted only by the adopting project's `ScenarioTarget`.
//!
//! Only the **syntactic** normalization passes live in this crate — canonical
//! identifier renaming and commutativity reordering, both pure functions over
//! the envelope. The replay-driven passes (prefix minimization, the determinism
//! check) need to execute a scenario to know whether a step mattered, so they
//! live in `verify-runner` alongside the replay engine. Putting them here would
//! be a dependency cycle.
//!
//! Contents arrive in M2.
