//! Project-populated replay runner. Finite replay is not a model-checking proof.

mod cli;
mod corpus;
mod normalize;
mod registry;
mod replay;

pub use cli::run_cli;
pub use corpus::{CorpusError, InsertResult, insert_scenario};
pub use normalize::{Normalization, NormalizationError, NormalizationOptions};
pub use registry::{RegistrationError, TargetRegistry, TargetResolutionError};
pub use replay::{Observation, ReplayLimits, ReplayReport, TraceVerdict};
