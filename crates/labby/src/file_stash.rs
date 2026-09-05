//! Process-owned persistence foundation for the principal-scoped File Stash.
mod runtime;
mod schema;
mod store;
#[allow(unused_imports)]
pub(crate) use crate::access::AccessPrincipalId as PrincipalId;
#[allow(unused_imports)]
pub(crate) use runtime::{FileStashBlockedReason, FileStashRuntime, FileStashStatus};
#[allow(unused_imports)]
pub(crate) use store::{FileStashStore, FileStashStoreError};
