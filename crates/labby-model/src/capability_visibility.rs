use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use verify_core::{InvariantId, InvariantResult, ScenarioError, ScenarioTarget, StepOutcome};

/// Stable abstract capability identifiers shared by the model's health views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityCode {
    RemoteHostNotAllowed,
    HostAllowlistWildcardIgnored,
    HostValidationDisabled,
    GatewayFeatureUnavailable,
    ArtifactsUnavailable,
    UsageTelemetryUnavailable,
}

/// Startup lifecycle relevant to capability visibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Configuring,
    Running,
    Blocked,
}

/// Transport-free state for configuration guards and runtime degradation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityVisibilityState {
    pub phase: Phase,
    pub remote_bind: bool,
    pub exact_remote_host: bool,
    pub wildcard_host: bool,
    pub host_validation_disabled: bool,
    pub gateway_feature_disabled: bool,
    pub upstream_configured: bool,
    pub protected_route_configured: bool,
    pub degraded: BTreeSet<CapabilityCode>,
    pub doctor_visible: BTreeSet<CapabilityCode>,
    pub readiness_visible: BTreeSet<CapabilityCode>,
    pub operator_detail: BTreeMap<CapabilityCode, String>,
    pub fatal_guard: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InitialState {}

/// Finite configuration/runtime events relevant to capability honesty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    BindRemote,
    AllowExactRemoteHost,
    AllowWildcardHost,
    DisableHostValidation,
    DisableGatewayFeature,
    ConfigureUpstream,
    ConfigureProtectedRoute,
    Start,
    RecordRuntimeDegradation { code: RuntimeCapability },
}

/// Optional subsystems that may fail after startup while the process keeps serving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapability {
    Artifacts,
    UsageTelemetry,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CapabilityVisibilityModel;

impl CapabilityVisibilityModel {
    fn rejected(reason: impl Into<String>) -> StepOutcome {
        StepOutcome::Rejected {
            reason: reason.into(),
        }
    }

    fn require_configuring(state: &CapabilityVisibilityState) -> Result<(), StepOutcome> {
        (state.phase == Phase::Configuring)
            .then_some(())
            .ok_or_else(|| Self::rejected("configuration is immutable after startup is attempted"))
    }

    fn project_degradation(
        state: &mut CapabilityVisibilityState,
        code: CapabilityCode,
        detail: impl Into<String>,
    ) {
        let detail = detail.into();
        state.degraded.insert(code);
        state.doctor_visible.insert(code);
        state.readiness_visible.insert(code);
        state.operator_detail.insert(code, detail);
    }

    fn start(state: &mut CapabilityVisibilityState) -> StepOutcome {
        if Self::require_configuring(state).is_err() {
            return Self::rejected("startup was already attempted");
        }

        if state.gateway_feature_disabled && state.protected_route_configured {
            state.phase = Phase::Blocked;
            state.fatal_guard =
                Some("protected MCP routes require a gateway-enabled Labby build".into());
            return StepOutcome::Applied {};
        }

        if state.wildcard_host {
            Self::project_degradation(
                state,
                CapabilityCode::HostAllowlistWildcardIgnored,
                "wildcard allowed-host entries are ignored; configure the exact client authority",
            );
        }
        if state.host_validation_disabled {
            Self::project_degradation(
                state,
                CapabilityCode::HostValidationDisabled,
                "DNS-rebinding host validation is disabled by a test-only escape hatch",
            );
        }
        if state.remote_bind && !state.exact_remote_host {
            Self::project_degradation(
                state,
                CapabilityCode::RemoteHostNotAllowed,
                "remote HTTP bind has no exact public/allowed Host, so remote requests will be rejected",
            );
        }
        if state.gateway_feature_disabled && state.upstream_configured {
            Self::project_degradation(
                state,
                CapabilityCode::GatewayFeatureUnavailable,
                "configured gateway upstreams are unavailable in a build without the gateway feature",
            );
        }
        state.phase = Phase::Running;
        StepOutcome::Applied {}
    }

    fn record_runtime(
        state: &mut CapabilityVisibilityState,
        runtime: RuntimeCapability,
    ) -> StepOutcome {
        if state.phase != Phase::Running {
            return Self::rejected("runtime degradation can only occur after successful startup");
        }
        match runtime {
            RuntimeCapability::Artifacts => Self::project_degradation(
                state,
                CapabilityCode::ArtifactsUnavailable,
                "Artifact services are unavailable; repair the configured source/storage runtime",
            ),
            RuntimeCapability::UsageTelemetry => Self::project_degradation(
                state,
                CapabilityCode::UsageTelemetryUnavailable,
                "usage telemetry persistence is unavailable; repair the telemetry store",
            ),
        }
        StepOutcome::Applied {}
    }
}

impl ScenarioTarget for CapabilityVisibilityModel {
    type State = CapabilityVisibilityState;
    type Step = Step;

    fn init(&self, initial: &Value) -> Result<Self::State, ScenarioError> {
        serde_json::from_value::<InitialState>(initial.clone())
            .map_err(|error| ScenarioError::InvalidInitial(error.to_string()))?;
        Ok(CapabilityVisibilityState::default())
    }

    fn apply(
        &self,
        state: &mut Self::State,
        step: &Self::Step,
    ) -> Result<StepOutcome, ScenarioError> {
        let outcome = match step {
            Step::Start => Self::start(state),
            Step::RecordRuntimeDegradation { code } => Self::record_runtime(state, *code),
            Step::BindRemote
            | Step::AllowExactRemoteHost
            | Step::AllowWildcardHost
            | Step::DisableHostValidation
            | Step::DisableGatewayFeature
            | Step::ConfigureUpstream
            | Step::ConfigureProtectedRoute => {
                if let Err(rejected) = Self::require_configuring(state) {
                    rejected
                } else {
                    match step {
                        Step::BindRemote => state.remote_bind = true,
                        Step::AllowExactRemoteHost => state.exact_remote_host = true,
                        Step::AllowWildcardHost => state.wildcard_host = true,
                        Step::DisableHostValidation => state.host_validation_disabled = true,
                        Step::DisableGatewayFeature => state.gateway_feature_disabled = true,
                        Step::ConfigureUpstream => state.upstream_configured = true,
                        Step::ConfigureProtectedRoute => state.protected_route_configured = true,
                        Step::Start | Step::RecordRuntimeDegradation { .. } => unreachable!(),
                    }
                    StepOutcome::Applied {}
                }
            }
        };
        Ok(outcome)
    }

    fn check(
        &self,
        id: &InvariantId,
        state: &Self::State,
    ) -> Result<InvariantResult, ScenarioError> {
        let violation = match id.as_str() {
            "LABBY-CAP-001" => state
                .degraded
                .iter()
                .find(|code| !state.doctor_visible.contains(code))
                .map(|code| {
                    format!("{code:?} is degraded but absent from Doctor capability findings")
                }),
            "LABBY-CAP-002" => state
                .degraded
                .iter()
                .find(|code| !state.readiness_visible.contains(code))
                .map(|code| format!("{code:?} is degraded but absent from readiness codes")),
            "LABBY-CAP-003" => state.degraded.iter().find_map(|code| {
                state
                    .operator_detail
                    .get(code)
                    .is_none_or(|detail| detail.trim().is_empty())
                    .then(|| format!("{code:?} has no actionable operator detail"))
            }),
            "LABBY-CAP-004" => (state.phase == Phase::Running
                && state.gateway_feature_disabled
                && state.protected_route_configured)
                .then(|| "protected routes entered Running without gateway support".into()),
            _ => return Err(ScenarioError::UnknownInvariant(id.clone())),
        };
        Ok(violation.map_or(InvariantResult::Holds {}, |reason| {
            InvariantResult::Violated { reason }
        }))
    }

    fn canonicalize(
        &self,
        initial: &Value,
        steps: &[Self::Step],
    ) -> Result<(Value, Vec<Self::Step>), ScenarioError> {
        drop(self.init(initial)?);
        Ok((initial.clone(), steps.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invariant(value: &str) -> InvariantId {
        InvariantId::try_from(value.to_owned()).unwrap()
    }

    #[test]
    fn remote_guard_degradation_is_visible_everywhere_with_detail() {
        let model = CapabilityVisibilityModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        for step in [Step::BindRemote, Step::Start] {
            assert_eq!(
                model.apply(&mut state, &step).unwrap(),
                StepOutcome::Applied {}
            );
        }
        assert!(
            state
                .degraded
                .contains(&CapabilityCode::RemoteHostNotAllowed)
        );
        for id in ["LABBY-CAP-001", "LABBY-CAP-002", "LABBY-CAP-003"] {
            assert_eq!(
                model.check(&invariant(id), &state).unwrap(),
                InvariantResult::Holds {}
            );
        }
    }

    #[test]
    fn fatal_gateway_mismatch_blocks_startup() {
        let model = CapabilityVisibilityModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        for step in [
            Step::DisableGatewayFeature,
            Step::ConfigureProtectedRoute,
            Step::Start,
        ] {
            drop(model.apply(&mut state, &step).unwrap());
        }
        assert_eq!(state.phase, Phase::Blocked);
        assert!(state.fatal_guard.is_some());
        assert_eq!(
            model.check(&invariant("LABBY-CAP-004"), &state).unwrap(),
            InvariantResult::Holds {}
        );
    }

    #[test]
    fn runtime_degradation_projects_to_both_health_surfaces() {
        let model = CapabilityVisibilityModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        drop(model.apply(&mut state, &Step::Start).unwrap());
        drop(
            model
                .apply(
                    &mut state,
                    &Step::RecordRuntimeDegradation {
                        code: RuntimeCapability::Artifacts,
                    },
                )
                .unwrap(),
        );
        assert!(
            state
                .doctor_visible
                .contains(&CapabilityCode::ArtifactsUnavailable)
        );
        assert!(
            state
                .readiness_visible
                .contains(&CapabilityCode::ArtifactsUnavailable)
        );
    }
}
