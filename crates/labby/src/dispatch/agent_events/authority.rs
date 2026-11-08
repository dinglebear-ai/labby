use std::sync::Arc;

use futures::future::BoxFuture;
use labby_auth::middleware::{ProjectSessionRevalidationError, ProjectSessionRevalidator as _};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventAuthorityDecision {
    Current,
    Revoked,
    RefreshRequired,
}

pub(crate) trait EventAuthorizer: Send + Sync {
    fn check(&self) -> BoxFuture<'_, EventAuthorityDecision>;
}

pub(crate) enum EventAuthority {
    /// The HTTP auth layer owns a process-lifetime static token configuration.
    /// Rotation restarts that process and discards every authority key.
    StaticBearer,
    OAuth(labby_auth::auth_context::VerifiedOAuthGrant),
    Product(
        Arc<crate::access::AccessCredentialAdapter>,
        Box<labby_auth::types::ProjectSessionBinding>,
    ),
}

pub(crate) async fn require_current(
    check: &dyn EventAuthorizer,
) -> Result<(), crate::dispatch::codemode_notices::NoticeError> {
    use crate::dispatch::codemode_notices::NoticeError;
    match tokio::time::timeout(std::time::Duration::from_secs(2), check.check()).await {
        Ok(EventAuthorityDecision::Current) => Ok(()),
        Ok(EventAuthorityDecision::Revoked) => Err(NoticeError::Identity),
        Ok(EventAuthorityDecision::RefreshRequired) | Err(_) => Err(NoticeError::Unavailable),
    }
}

impl EventAuthorizer for EventAuthority {
    fn check(&self) -> BoxFuture<'_, EventAuthorityDecision> {
        use EventAuthorityDecision::{Current, RefreshRequired, Revoked};
        Box::pin(async {
            match self {
                Self::StaticBearer => Current,
                Self::OAuth(grant) => match grant.is_current().await {
                    Ok(true) => Current,
                    Ok(false) => Revoked,
                    Err(_) => RefreshRequired,
                },
                Self::Product(adapter, binding) => match adapter.revalidate(binding).await {
                    Ok(_) => Current,
                    Err(ProjectSessionRevalidationError::Denied) => Revoked,
                    Err(ProjectSessionRevalidationError::Unavailable) => RefreshRequired,
                },
            }
        })
    }
}
