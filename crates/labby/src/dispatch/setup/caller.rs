//! Caller authority for setup actions that stage or commit environment keys.
//!
//! An allowlist entry or a durable `platform.manage` grant is enough to reach
//! `requires_admin` setup actions, but it must not be enough to rewrite the
//! gateway's own authentication configuration: whoever controls
//! `LABBY_MCP_HTTP_TOKEN`, `LABBY_AUTH_ADMIN_EMAIL`, or the identity-provider
//! secrets controls every later session. Keys whose generated catalog service
//! is `auth` are therefore reserved for the host operator. Surfaces only
//! report transport evidence; this module owns the decision.

use crate::dispatch::error::ToolError;

/// Catalog service whose environment keys configure Labby authentication.
const AUTH_SERVICE: &str = "auth";

/// Who is asking setup to stage or commit environment changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupCaller {
    /// The host operator: a loopback caller holding the local capability, an
    /// operator transport credential (static bearer or Unix peer), or the
    /// browser session of the configured admin email.
    Operator,
    /// Every other caller, including allowlisted members elevated from durable
    /// platform authority, OAuth bearer tokens, and MCP.
    Delegated,
}

/// Transport facts a surface reports so shared dispatch can classify the caller.
#[derive(Debug, Clone, Copy, Default)]
pub struct SetupCallerEvidence<'a> {
    /// Loopback socket peer and loopback `Host` header.
    pub local_capability: bool,
    /// Authenticated by an operator-held transport credential.
    pub operator_credential: bool,
    /// Email on the authenticated browser session, if the caller used one.
    pub session_email: Option<&'a str>,
    /// The configured admin email, if authentication is configured.
    pub configured_admin_email: Option<&'a str>,
}

impl SetupCaller {
    /// Classify a caller from surface-reported evidence. Anything not proven
    /// to be the operator is delegated.
    #[must_use]
    pub fn classify(evidence: SetupCallerEvidence<'_>) -> Self {
        let configured_admin_session =
            match (evidence.session_email, evidence.configured_admin_email) {
                (Some(email), Some(admin)) => {
                    !admin.is_empty() && email.eq_ignore_ascii_case(admin)
                }
                _ => false,
            };
        if evidence.local_capability || evidence.operator_credential || configured_admin_session {
            Self::Operator
        } else {
            Self::Delegated
        }
    }

    /// Refuse the first authentication key a delegated caller tries to stage
    /// or commit.
    pub(super) fn ensure_may_write<'k>(
        self,
        keys: impl IntoIterator<Item = &'k str>,
    ) -> Result<(), ToolError> {
        if self == Self::Operator {
            return Ok(());
        }
        let schema = super::settings::env_schema()?;
        for key in keys {
            if schema
                .iter()
                .any(|spec| spec.key == key && spec.service == AUTH_SERVICE)
            {
                return Err(ToolError::Forbidden {
                    message: format!(
                        "environment key `{key}` configures Labby authentication; only the local \
                         operator or the configured admin may stage or commit it"
                    ),
                    required_scopes: Vec::new(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_operator_evidence_classifies_as_operator() {
        let admin = Some("owner@example.com");
        for (evidence, expected) in [
            (SetupCallerEvidence::default(), SetupCaller::Delegated),
            (
                SetupCallerEvidence {
                    local_capability: true,
                    ..Default::default()
                },
                SetupCaller::Operator,
            ),
            (
                SetupCallerEvidence {
                    operator_credential: true,
                    ..Default::default()
                },
                SetupCaller::Operator,
            ),
            (
                SetupCallerEvidence {
                    session_email: Some("OWNER@EXAMPLE.COM"),
                    configured_admin_email: admin,
                    ..Default::default()
                },
                SetupCaller::Operator,
            ),
            (
                SetupCallerEvidence {
                    session_email: Some("colleague@example.com"),
                    configured_admin_email: admin,
                    ..Default::default()
                },
                SetupCaller::Delegated,
            ),
            (
                SetupCallerEvidence {
                    session_email: Some(""),
                    configured_admin_email: Some(""),
                    ..Default::default()
                },
                SetupCaller::Delegated,
            ),
            (
                SetupCallerEvidence {
                    session_email: Some("owner@example.com"),
                    configured_admin_email: None,
                    ..Default::default()
                },
                SetupCaller::Delegated,
            ),
        ] {
            assert_eq!(SetupCaller::classify(evidence), expected, "{evidence:?}");
        }
    }

    #[test]
    fn every_auth_catalog_key_is_reserved_for_the_operator() {
        let schema = super::super::settings::env_schema().unwrap();
        let auth_keys: Vec<&str> = schema
            .iter()
            .filter(|spec| spec.service == AUTH_SERVICE)
            .map(|spec| spec.key.as_str())
            .collect();
        for required in [
            "LABBY_MCP_HTTP_TOKEN",
            "LABBY_AUTH_ADMIN_EMAIL",
            "LABBY_GOOGLE_CLIENT_SECRET",
        ] {
            assert!(
                auth_keys.contains(&required),
                "{required} must be an auth key"
            );
        }
        for key in &auth_keys {
            let error = SetupCaller::Delegated
                .ensure_may_write([*key])
                .expect_err(key);
            assert_eq!(error.kind(), "forbidden", "{key}");
            SetupCaller::Operator.ensure_may_write([*key]).unwrap();
        }
        SetupCaller::Delegated
            .ensure_may_write(["LABBY_LOG"])
            .expect("non-auth keys stay writable by delegated admins");
    }
}
