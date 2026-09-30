//! Executable C1 DTO proposal, deliberately test-only pending Gate 0/A integration.
//! SDK responses are the single source of protocol metadata. No peer, credential,
//! session, authorization method, or second extension map is retained here.
use rmcp::model::{
    CacheScope, DiscoverResult, Implementation, InitializeResult, MetaObject, ProtocolVersion,
    ResultType, ServerCapabilities,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ObservationContext {
    pub(super) upstream_name: String,
    pub(super) config_fingerprint: String,
    pub(super) observation_scope: String,
    pub(super) catalog_fingerprint: Option<String>,
    pub(super) observed_at_unix_ms: u64,
    // Copied from the owning publication guard; never serialized or restored.
    pub(super) publication_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProtocolEra {
    Modern20260728,
    ExplicitLegacy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnapshotError {
    UnsupportedEra,
    EffectiveVersionNotAdvertised,
    IncompleteDiscovery,
    FreshnessOverflow,
}

#[derive(Clone)]
enum Observation {
    Discovery {
        result: DiscoverResult,
        effective_version: ProtocolVersion,
    },
    Legacy(InitializeResult),
}

#[derive(Clone)]
pub(super) struct ServerSnapshot {
    context: ObservationContext,
    observation: Observation,
    freshness_deadline_unix_ms: Option<u64>,
}

impl ServerSnapshot {
    pub(super) fn from_discovery(
        context: ObservationContext,
        result: DiscoverResult,
        effective_version: ProtocolVersion,
    ) -> Result<Self, SnapshotError> {
        if effective_version != ProtocolVersion::V_2026_07_28 {
            return Err(SnapshotError::UnsupportedEra);
        }
        if result.result_type != ResultType::COMPLETE {
            return Err(SnapshotError::IncompleteDiscovery);
        }
        if !result.supported_versions.contains(&effective_version) {
            return Err(SnapshotError::EffectiveVersionNotAdvertised);
        }
        let deadline = context
            .observed_at_unix_ms
            .checked_add(result.ttl_ms)
            .ok_or(SnapshotError::FreshnessOverflow)?;
        Ok(Self {
            context,
            observation: Observation::Discovery {
                result,
                effective_version,
            },
            freshness_deadline_unix_ms: Some(deadline),
        })
    }

    pub(super) fn from_legacy(
        context: ObservationContext,
        result: InitializeResult,
    ) -> Result<Self, SnapshotError> {
        if result.protocol_version == ProtocolVersion::V_2026_07_28
            || !ProtocolVersion::KNOWN_VERSIONS.contains(&result.protocol_version)
        {
            return Err(SnapshotError::UnsupportedEra);
        }
        Ok(Self {
            context,
            observation: Observation::Legacy(result),
            // Legacy initialization supplied no discovery cache guarantee.
            freshness_deadline_unix_ms: None,
        })
    }

    pub(super) fn context(&self) -> &ObservationContext {
        &self.context
    }

    pub(super) fn era(&self) -> ProtocolEra {
        match self.observation {
            Observation::Discovery { .. } => ProtocolEra::Modern20260728,
            Observation::Legacy(_) => ProtocolEra::ExplicitLegacy,
        }
    }

    pub(super) fn effective_version(&self) -> &ProtocolVersion {
        match &self.observation {
            Observation::Discovery {
                effective_version, ..
            } => effective_version,
            Observation::Legacy(result) => &result.protocol_version,
        }
    }

    pub(super) fn supported_versions(&self) -> Option<&[ProtocolVersion]> {
        match &self.observation {
            Observation::Discovery { result, .. } => Some(&result.supported_versions),
            // One negotiated legacy version is not a supported-version inventory.
            Observation::Legacy(_) => None,
        }
    }

    pub(super) fn capabilities(&self) -> &ServerCapabilities {
        match &self.observation {
            Observation::Discovery { result, .. } => &result.capabilities,
            Observation::Legacy(result) => &result.capabilities,
        }
    }

    pub(super) fn server_info(&self) -> Option<Implementation> {
        match &self.observation {
            Observation::Discovery { result, .. } => result.server_info(),
            Observation::Legacy(result) => Some(result.server_info.clone()),
        }
    }

    pub(super) fn instructions(&self) -> Option<&str> {
        match &self.observation {
            Observation::Discovery { result, .. } => result.instructions.as_deref(),
            Observation::Legacy(result) => result.instructions.as_deref(),
        }
    }

    pub(super) fn meta(&self) -> Option<&MetaObject> {
        match &self.observation {
            Observation::Discovery { result, .. } => result.meta.as_ref(),
            Observation::Legacy(result) => result.meta.as_ref(),
        }
    }

    pub(super) fn cache_hints(&self) -> Option<(u64, &CacheScope)> {
        self.raw_discovery()
            .map(|result| (result.ttl_ms, &result.cache_scope))
    }

    pub(super) fn freshness_deadline_unix_ms(&self) -> Option<u64> {
        self.freshness_deadline_unix_ms
    }

    pub(super) fn is_fresh_at(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.context.observed_at_unix_ms
            && self
                .freshness_deadline_unix_ms
                .is_some_and(|deadline| now_unix_ms < deadline)
    }

    pub(super) fn raw_discovery(&self) -> Option<&DiscoverResult> {
        match &self.observation {
            Observation::Discovery { result, .. } => Some(result),
            Observation::Legacy(_) => None,
        }
    }
}
