//! Invariant catalog, verdict, and target vocabulary for the verification
//! toolkit.
//!
//! This crate is the dependency leaf. It is **transport-free, filesystem-free,
//! and environment-free** — the same discipline `labby-primitives` and
//! `labby-apis` carry in the product workspace. It may depend on `serde` and
//! `thiserror` and nothing else.
//!
//! It also holds no domain vocabulary. There is no `upstream`, no `mount`, no
//! `session` here, and there never will be: a change needed to support one
//! adopting project's domain is a design bug in this crate rather than a
//! feature request. See `docs/plans/verification-toolkit/SPEC.md` §2.
//!
//! Contents arrive in M1: `InvariantId`, `Kind`, `Severity`, `Catalog` and its
//! validation rules, `Verdict`, `Backend`, and `ScenarioTarget`.
