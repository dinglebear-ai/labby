//! Shared access-store failure mapping for authenticated product dispatchers.
//!
//! Every surface adapter and dispatcher that talks to the access store maps its
//! typed failures through this module so the three surfaces agree on:
//!
//! - which failures are non-enumerating denials (`forbidden`);
//! - which failures are caller-fixable input problems (`invalid_param`);
//! - which failures are store lifecycle outages (`service_unavailable`) that
//!   must be logged server-side with their typed cause but never leaked
//!   verbatim to the caller;
//! - which failures are the deterministic owner-setup gate
//!   (`access_setup_required`) rather than an outage.
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
/// envelope. Neither state is an authorization decision.
///
/// - `SetupRequired` is a deterministic setup gate, not an outage: the store
///   has never been initialized, nothing ran, and retrying cannot help until
///   the operator completes owner setup. It maps to `access_setup_required`.
/// - `Blocked` and lifecycle failures are real outages (`service_unavailable`).
pub(crate) fn map_runtime_error(service: &'static str, error: AccessRuntimeError) -> ToolError {
    map_runtime_error_with_action(service, None, error)
}

/// [`map_runtime_error`] for a caller that knows the product action, so the
/// server-side log line names the action that hit the lifecycle gate.
pub(crate) fn map_action_runtime_error(
    service: &'static str,
    action: &str,
    error: AccessRuntimeError,
) -> ToolError {
    map_runtime_error_with_action(service, Some(action), error)
}

fn map_runtime_error_with_action(
    service: &'static str,
    action: Option<&str>,
    error: AccessRuntimeError,
) -> ToolError {
    match error {
        AccessRuntimeError::SetupRequired(reason) => {
            let reason_code = match reason {
                AccessSetupReason::Missing => "missing",
                AccessSetupReason::Uninitialized => "uninitialized",
                AccessSetupReason::ProofPending => "proof_pending",
            };
            tracing::warn!(
                service,
                action,
                reason = reason_code,
                kind = ACCESS_SETUP_REQUIRED_KIND,
                "access store setup is required; complete owner setup (browser owner setup, or `labby setup` then restart) or finish the pending access bootstrap"
            );
            setup_required(reason)
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
                    action,
                    reason,
                    kind = "service_unavailable",
                    "access store is blocked; operator action required"
                );
            } else {
                tracing::warn!(
                    service,
                    action,
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
                action,
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

/// Stable kind for a never-initialized durable access store. Classified in
/// `labby_runtime::agent_error` as a `validation`-origin setup gate with
/// `start_dependency` recovery and no side effects.
const ACCESS_SETUP_REQUIRED_KIND: &str = "access_setup_required";

/// The caller-facing setup gate. The message is a fixed string addressed to
/// the Labby server's operator that names the remediation for the observed
/// setup state; the typed reason also stays in the server log.
///
/// `AccessRuntime` only re-observes store health at process start, so a
/// bearer-only install that runs `labby setup` out of process must restart the
/// serving Labby process. Browser owner setup and consuming a pending
/// bootstrap both promote the running process directly.
fn setup_required(reason: AccessSetupReason) -> ToolError {
    let message = match reason {
        AccessSetupReason::Missing | AccessSetupReason::Uninitialized => {
            "access setup is required: this Labby server's access store has not been \
             initialized, so nothing ran. Ask the operator of the Labby server to complete \
             owner setup: installs with any OAuth provider (including bearer plus OAuth) \
             complete browser owner setup in the Labby web UI; bearer-token-only installs \
             run `labby setup` on the Labby server host and then restart the serving Labby \
             process. Retry after setup succeeds."
        }
        AccessSetupReason::ProofPending => {
            "access setup is required: an owner access bootstrap was prepared on this Labby \
             server but not completed, so nothing ran. Ask the operator of the Labby server to \
             finish it with `labby setup access-bootstrap consume --prepare-id <id>`, or to \
             remove it with `labby setup access-bootstrap cleanup --prepare-id <id>` while \
             Labby is stopped and then complete owner setup. Retry after setup succeeds."
        }
    };
    ToolError::Sdk {
        sdk_kind: ACCESS_SETUP_REQUIRED_KIND.to_owned(),
        message: message.to_owned(),
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

    /// A never-initialized access store is a deterministic setup gate: it is
    /// not temporary, nothing ran, the upstream transport was never touched,
    /// and the envelope names the owner-setup remediation.
    #[test]
    fn setup_required_is_a_non_retryable_setup_gate() {
        for reason in [AccessSetupReason::Missing, AccessSetupReason::Uninitialized] {
            let mapped = map_runtime_error("gateway", AccessRuntimeError::SetupRequired(reason));
            assert_eq!(mapped.kind(), "access_setup_required", "{reason:?}");
            let envelope = mapped.to_agent_value();
            assert_eq!(envelope["kind"], "access_setup_required");
            assert_eq!(envelope["origin"], "validation");
            assert_eq!(envelope["side_effects"], "none_expected");
            assert_eq!(envelope["recovery"]["action"], "start_dependency");
            assert_eq!(envelope["recovery"]["same_arguments"], "never");
            let message = envelope["message"].as_str().unwrap();
            assert!(message.contains("access setup is required"), "{message}");
            assert!(message.contains("`labby setup`"), "{message}");
            assert!(!message.contains("temporarily"), "{message}");
        }
    }

    /// The serving process only re-observes store health at startup, so a
    /// bearer install that ran `labby setup` out of process must restart the
    /// serving Labby process. The message addresses the server operator, and
    /// any OAuth-including install (`--auth both` too) is sent to browser
    /// owner setup, which promotes the running process without a restart.
    #[test]
    fn setup_required_message_asks_the_operator_and_names_the_restart() {
        for reason in [AccessSetupReason::Missing, AccessSetupReason::Uninitialized] {
            let mapped = map_runtime_error("gateway", AccessRuntimeError::SetupRequired(reason));
            let message = mapped.user_message();
            for phrase in [
                "access setup is required",
                "Ask the operator of the Labby server",
                "`labby setup`",
                "restart the serving Labby process",
                "OAuth provider",
                "browser owner setup",
            ] {
                assert!(message.contains(phrase), "{reason:?}: {phrase}: {message}");
            }
        }
    }

    /// A prepared-but-unconsumed owner bootstrap refuses both `labby setup`
    /// and browser owner setup, so its guidance must name the pending
    /// bootstrap instead.
    #[test]
    fn proof_pending_setup_names_the_pending_bootstrap() {
        let mapped = map_runtime_error(
            "gateway",
            AccessRuntimeError::SetupRequired(AccessSetupReason::ProofPending),
        );
        assert_eq!(mapped.kind(), "access_setup_required");
        let message = mapped.user_message();
        for phrase in [
            "access setup is required",
            "Ask the operator of the Labby server",
            "labby setup access-bootstrap consume",
            "labby setup access-bootstrap cleanup",
        ] {
            assert!(message.contains(phrase), "{phrase}: {message}");
        }
        assert!(!message.contains("browser owner setup"), "{message}");
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
