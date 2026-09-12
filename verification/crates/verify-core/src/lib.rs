//! Invariant catalog, verdict, and target vocabulary for the verification
//! toolkit.
//!
//! This crate is the dependency leaf. It is **transport-free, filesystem-free,
//! and environment-free** — the same discipline `labby-primitives` and
//! `labby-apis` carry in the product workspace. It may depend on `serde`,
//! `toml`, and `thiserror`, and nothing else.
//!
//! It also holds no domain vocabulary. There is no `upstream`, no `mount`, no
//! `session` here, and there never will be: a change needed to support one
//! adopting project's domain is a design bug in this crate rather than a
//! feature request. See `docs/plans/verification-toolkit/SPEC.md` §2.
//!
//! # What lives here and why
//!
//! - [`invariant`] — identity ([`InvariantId`]) and classification. Ids are
//!   permanently stable; retirement never frees one.
//! - [`catalog`] — the catalog format and the validation rules that stop a
//!   catalog from looking fine while checking nothing.
//! - [`verdict`] — outcomes, keeping `Bounded` distinct from `Verified` and
//!   `Skipped` distinct from `Error`.
//! - [`backend`] — the [`Backend`] contract and the capability vocabulary that
//!   gates which backends may claim which kinds of property. Defined here, not
//!   in the runner, so that `verify-runner` depends on no backend crate.
//! - [`target`] — [`ScenarioTarget`], the interface an adopting project writes.

pub mod backend;
pub mod catalog;
pub mod invariant;
pub mod target;
pub mod verdict;

pub use backend::{Availability, Backend, BackendId, Capabilities};
pub use catalog::{Catalog, CatalogError, CatalogParseError, Invariant};
pub use invariant::{InvariantId, Kind, Severity, Status};
pub use target::{ScenarioError, ScenarioTarget, StepOutcome};
pub use verdict::{Bound, InvariantResult, Verdict};
