//! Shared access-store failure mapping for authenticated product dispatchers.
//!
//! Every surface adapter and dispatcher that talks to the access store maps its
//! typed failures through this module so the three surfaces agree on:
//!
//! - which failures are non-enumerating denials (`forbidden`);
//! - which failures are caller-fixable input problems (`invalid_param`);
//! - which failures are store lifecycle outages (`service_unavailable`) that
//!   must be logged server-side with their typed cause but never leaked
//!   verbatim to the caller.
//!
//! The messages returned here are fixed strings. The typed cause is logged
//! through `tracing` (WARN for outages that heal on their own, ERROR for
//! integrity failures that require operator action) and never enters the
//! response envelope.
//!
//! This module is the canonical `AccessStoreError` → `ToolError` map for the
//! product crate. `access/error.rs` deliberately stays a pure error vocabulary
//! with no surface-envelope knowledge, so the map lives here at the dispatch
//! boundary and every dispatcher/adapter delegates to it.

use crate::access::{AccessBlockedReason, AccessRuntimeError, AccessSetupReason, AccessStoreError};
use crate::dispatch::error::ToolError;

/// Map a typed access-store failure to the shared surface error envelope.
///
/// `service` is only used for the server-side log line. `denied` produces the
/// service's non-enumerating denial so callers cannot distinguish "does not
/// exist" from "not yours" across the four denial-class failures.
pub(crate) fn map_store_error(
    service: &'static str,
    error: AccessStoreError,
    denied: fn() -> ToolError,
) -> ToolError {
    use AccessStoreError as E;
    match error {
        E::NotAuthorized
        | E::IdentityUnavailable
        | E::ProjectAccessUnavailable
        | E::TeamUnavailable
        | E::ForeignKeyViolation => denied(),
        E::InvalidTeamInput | E::InvalidProjectLoadoutInput | E::InvalidBootstrapInput => {
            ToolError::InvalidParam {
                message: "invalid parameter `params`".to_owned(),
                param: "params".to_owned(),
            }
        }
        // The caller already proved Team management authority; a missing
        // binding is a caller-fixable not-found inside that Team, never an
        // enumeration of other Teams' bindings.
        E::TeamCredentialBindingUnavailable => ToolError::Sdk {
            sdk_kind: "not_found".to_owned(),
            message: "no Team Gateway credential binding exists for that upstream".to_owned(),
        },
        E::LastActiveTeamOwner => ToolError::Conflict {
            message: "team must retain an active owner".to_owned(),
            existing_id: "team_owner".to_owned(),
        },
        E::ProjectLoadoutConflict => ToolError::Conflict {
            message: "project already has a different loadout assignment".to_owned(),
            existing_id: "project_loadout".to_owned(),
        },
        E::BootstrapConflict => ToolError::Conflict {
            message: "owner bootstrap conflicts with existing state".to_owned(),
            existing_id: "bootstrap_owner".to_owned(),
        },
        E::MalformedVocabulary
        | E::IntegrityViolation { .. }
        | E::ProjectionWatermarkRegressed
        | E::Corrupt => {
            tracing::error!(
                service,
                cause = %error,
                kind = "service_unavailable",
                "access store integrity failure; operator action required"
            );
            unavailable()
        }
        E::Locked
        | E::DiskFull
        | E::ReadOnly
        | E::InsecurePath { .. }
        | E::MissingParent { .. }
        | E::InsecurePermissions { .. }
        | E::UnsupportedSchema { .. }
        | E::MigrationApprovalRequired { .. }
        | E::MigrationEvidenceInvalid { .. }
        | E::Unavailable(_) => {
            tracing::warn!(
                service,
                cause = %error,
                kind = "service_unavailable",
                "access store is unavailable"
            );
            unavailable()
        }
    }
}

/// Map a runtime-lifecycle failure (`AccessRuntime::store()`) to the shared
/// envelope. Setup-required and blocked states are outages from the caller's
/// point of view, never authorization decisions.
pub(crate) fn map_runtime_error(service: &'static str, error: AccessRuntimeError) -> ToolError {
    match error {
        AccessRuntimeError::SetupRequired(reason) => {
            let reason = match reason {
                AccessSetupReason::Missing => "missing",
                AccessSetupReason::Uninitialized => "uninitialized",
            };
            tracing::warn!(
                service,
                reason,
                kind = "service_unavailable",
                "access store setup is required"
            );
            unavailable()
        }
        AccessRuntimeError::Blocked(reason) => {
            let (level_error, reason) = match reason {
                AccessBlockedReason::Insecure => (true, "insecure"),
                AccessBlockedReason::Corrupt => (true, "corrupt"),
                AccessBlockedReason::NewerSchema => (true, "newer_schema"),
                AccessBlockedReason::Locked => (false, "locked"),
                AccessBlockedReason::ReadOnly => (false, "read_only"),
                AccessBlockedReason::Unavailable => (false, "unavailable"),
            };
            if level_error {
                tracing::error!(
                    service,
                    reason,
                    kind = "service_unavailable",
                    "access store is blocked; operator action required"
                );
            } else {
                tracing::warn!(
                    service,
                    reason,
                    kind = "service_unavailable",
                    "access store is blocked"
                );
            }
            unavailable()
        }
        AccessRuntimeError::BootstrapConflict
        | AccessRuntimeError::InvalidBootstrapInput
        | AccessRuntimeError::LifecycleUnavailable => {
            tracing::warn!(
                service,
                cause = %error,
                kind = "service_unavailable",
                "access runtime lifecycle is unavailable"
            );
            unavailable()
        }
    }
}

fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".to_owned(),
        message: "access store is unavailable".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn denied() -> ToolError {
        ToolError::Forbidden {
            message: "access denied".into(),
            required_scopes: Vec::new(),
        }
    }

    #[test]
    fn denial_class_collapses_to_one_non_enumerating_error() {
        for error in [
            AccessStoreError::NotAuthorized,
            AccessStoreError::IdentityUnavailable,
            AccessStoreError::ProjectAccessUnavailable,
            AccessStoreError::TeamUnavailable,
            AccessStoreError::ForeignKeyViolation,
        ] {
            let mapped = map_store_error("test", error, denied);
            assert_eq!(mapped.kind(), "forbidden");
            let envelope: Value = serde_json::from_str(&mapped.to_string()).unwrap();
            assert_eq!(envelope["message"], "access denied");
        }
    }

    #[test]
    fn lifecycle_failures_are_outages_with_fixed_messages() {
        for error in [
            AccessStoreError::Locked,
            AccessStoreError::Corrupt,
            AccessStoreError::DiskFull,
            AccessStoreError::ReadOnly,
            AccessStoreError::MalformedVocabulary,
            AccessStoreError::IntegrityViolation {
                check: "schema_manifest",
            },
            AccessStoreError::ProjectionWatermarkRegressed,
            AccessStoreError::Unavailable("sqlite: /secret/path/access.db busy".into()),
            AccessStoreError::InsecurePath {
                path: "/secret/path".into(),
            },
        ] {
            let mapped = map_store_error("test", error, denied);
            assert_eq!(mapped.kind(), "service_unavailable");
            let text = mapped.to_string();
            assert!(!text.contains("/secret"), "{text}");
            assert!(!text.contains("sqlite"), "{text}");
        }
        for error in [
            AccessRuntimeError::SetupRequired(AccessSetupReason::Missing),
            AccessRuntimeError::Blocked(AccessBlockedReason::Corrupt),
            AccessRuntimeError::Blocked(AccessBlockedReason::Locked),
            AccessRuntimeError::LifecycleUnavailable,
        ] {
            assert_eq!(
                map_runtime_error("test", error).kind(),
                "service_unavailable"
            );
        }
    }

    #[test]
    fn input_problems_are_caller_fixable() {
        assert_eq!(
            map_store_error("test", AccessStoreError::InvalidTeamInput, denied).kind(),
            "invalid_param"
        );
        assert_eq!(
            map_store_error("test", AccessStoreError::LastActiveTeamOwner, denied).kind(),
            "conflict"
        );
        assert_eq!(
            map_store_error(
                "test",
                AccessStoreError::TeamCredentialBindingUnavailable,
                denied
            )
            .kind(),
            "not_found"
        );
    }
}
