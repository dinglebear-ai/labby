use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Fault {
    MissingRoute,
    AuthBypass,
    LeakedDescendant,
    SecretTraceRetention,
    WrongSurfaceFlag,
    IncorrectPolicyMetadata,
    RemoteFallback,
    DroppedRecoveryMetadata,
    HiddenUpstreamLeak,
    StaleCatalogOverwrite,
}

impl Fault {
    pub(crate) const ALL: [Self; 10] = [
        Self::MissingRoute,
        Self::AuthBypass,
        Self::LeakedDescendant,
        Self::SecretTraceRetention,
        Self::WrongSurfaceFlag,
        Self::IncorrectPolicyMetadata,
        Self::RemoteFallback,
        Self::DroppedRecoveryMetadata,
        Self::HiddenUpstreamLeak,
        Self::StaleCatalogOverwrite,
    ];
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::MissingRoute => "missing-route",
            Self::AuthBypass => "auth-bypass",
            Self::LeakedDescendant => "leaked-descendant",
            Self::SecretTraceRetention => "secret-trace-retention",
            Self::WrongSurfaceFlag => "wrong-surface-flag",
            Self::IncorrectPolicyMetadata => "incorrect-policy-metadata",
            Self::RemoteFallback => "remote-fallback",
            Self::DroppedRecoveryMetadata => "dropped-recovery-metadata",
            Self::HiddenUpstreamLeak => "hidden-upstream-leak",
            Self::StaleCatalogOverwrite => "stale-catalog-overwrite",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|fault| fault.name() == name)
    }
}

/// Test-process-only controller. Its opaque activation token cannot be
/// serialized, inherited by a subprocess, or constructed outside this module.
pub(crate) struct FaultControl {
    nonce: u64,
    active: BTreeSet<Fault>,
}
pub(crate) struct FaultHandle {
    nonce: u64,
    fault: Fault,
}

impl FaultControl {
    pub(crate) fn new() -> Self {
        Self {
            nonce: NEXT_NONCE.fetch_add(1, Ordering::Relaxed),
            active: BTreeSet::new(),
        }
    }
    pub(crate) fn activate(&mut self, names: &[&str]) -> Result<FaultHandle, String> {
        if names.len() != 1 {
            return Err("fault setup requires exactly one named fault".into());
        }
        let fault = Fault::parse(names[0])
            .ok_or_else(|| format!("unknown qualification fault: {}", names[0]))?;
        if !self.active.insert(fault) {
            return Err(format!(
                "qualification fault already active: {}",
                fault.name()
            ));
        }
        Ok(FaultHandle {
            nonce: self.nonce,
            fault,
        })
    }
    pub(crate) fn inject<T>(&self, handle: &FaultHandle, value: T) -> Result<T, String> {
        if handle.nonce != self.nonce || !self.active.contains(&handle.fault) {
            return Err("invalid private qualification fault handle".into());
        }
        Ok(value)
    }
    pub(crate) fn release(&mut self, handle: FaultHandle) -> Result<(), String> {
        if handle.nonce != self.nonce || !self.active.remove(&handle.fault) {
            return Err("invalid private qualification fault handle".into());
        }
        Ok(())
    }
}

pub(crate) fn detected(fault: Fault, detector: &str, detail: &str) -> String {
    format!(
        "qualification fault={} detector={} detail={detail}",
        fault.name(),
        detector
    )
}
