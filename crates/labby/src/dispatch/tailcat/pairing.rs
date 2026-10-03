use crate::config::{GatewayLoadoutConfig, ProtectedMcpRouteConfig};
use labby_primitives::product_credential::BoundAccessGrant;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq as _;

pub struct PairingRequest {
    pub origin: String,
    pub peer: String,
    pub upstream: String,
}
pub struct PendingPairing {
    request: PairingRequest,
    source: BoundAccessGrant,
    nonce: Option<[u8; 32]>,
    prepared_at: u64,
    started: Instant,
}
/// Consumed local approval; no credential or transport secret is issued here.
pub struct ApprovedPairing {
    request: PairingRequest,
    source: BoundAccessGrant,
    generation: String,
    expires_at: u64,
    approved_at: u64,
    started: Instant,
}
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("tailcat pairing denied")]
pub struct PairingError;

impl PendingPairing {
    /// Explicit secret accessor for authenticated approval exchange only.
    pub fn nonce(&self) -> Option<[u8; 32]> {
        self.nonce
    }
    /// Call only after explicit local approval and fresh native resolution.
    /// Even a denied approval burns the nonce; retry requires fresh pairing.
    pub fn approve(
        &mut self,
        nonce: &[u8; 32],
        request: &PairingRequest,
        current: &BoundAccessGrant,
        now: u64,
    ) -> Result<ApprovedPairing, PairingError> {
        let expected = self.nonce.take().ok_or(PairingError)?;
        if !bool::from(expected.ct_eq(nonce))
            || now < self.prepared_at
            || now.saturating_sub(self.prepared_at) >= 300
            || self.started.elapsed() >= Duration::from_mins(5)
            || current != &self.source
            || current.expires_at <= now
            || request.origin != self.request.origin
            || request.peer != self.request.peer
            || request.upstream != self.request.upstream
        {
            return Err(PairingError);
        }
        Ok(ApprovedPairing {
            request: PairingRequest {
                origin: self.request.origin.clone(),
                peer: self.request.peer.clone(),
                upstream: self.request.upstream.clone(),
            },
            source: self.source.clone(),
            generation: uuid::Uuid::new_v4().to_string(),
            expires_at: now.saturating_add(900).min(current.expires_at),
            approved_at: now,
            started: Instant::now(),
        })
    }
    /// The source must have just been resolved by the native credential adapter.
    /// This admission only narrows its published route; it never authenticates it.
    pub fn prepare(
        request: PairingRequest,
        source: BoundAccessGrant,
        installation: &str,
        route: &ProtectedMcpRouteConfig,
        loadout: &GatewayLoadoutConfig,
        now: u64,
    ) -> Result<Self, PairingError> {
        let origin = url::Url::parse(&request.origin).map_err(|_| PairingError)?;
        let peer = request.peer.strip_prefix("nodekey:").ok_or(PairingError)?;
        let target = route.gateway_subset_target().ok_or(PairingError)?;
        if origin.scheme() != "https"
            || origin.origin().ascii_serialization() != request.origin
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || peer.len() != 64
            || peer.bytes().all(|b| b == b'0')
            || !peer
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || installation.is_empty()
            || source.installation_id != installation
            || source.expires_at <= now
            || source.requires_admin
            || source.principal_id.is_empty()
            || source.credential_generation == 0
            || source.loadout_generation == 0
            || source.route_generation == 0
            || !route.enabled
            || route.name != source.route_id
            || route.public_resource() != source.resource
            || source.audience != source.resource
            || target.project_id.as_deref() != Some(source.project_id.as_str())
            || target.loadout.as_deref() != Some(source.loadout_id.as_str())
            || !target.upstreams.is_empty()
            || !target.services.is_empty()
            || target.expose_code_mode
            || loadout.name != source.loadout_id
            || request.upstream.is_empty()
            || loadout.upstreams != [request.upstream.clone()]
            || !loadout.services.is_empty()
            || loadout.expose_code_mode
            || !loadout.expose_tools
            || loadout.expose_resources
            || loadout.expose_prompts
            || loadout.expose_skills
        {
            return Err(PairingError);
        }
        let mut nonce = [0; 32];
        getrandom::fill(&mut nonce).map_err(|_| PairingError)?;
        Ok(Self {
            request,
            source,
            nonce: Some(nonce),
            prepared_at: now,
            started: Instant::now(),
        })
    }
}

impl ApprovedPairing {
    /// Current authority must be resolved afresh by the native adapter.
    pub fn authorize(
        &self,
        origin: &str,
        generation: &str,
        upstream: &str,
        current: &BoundAccessGrant,
        now: u64,
    ) -> Result<(), PairingError> {
        if now < self.approved_at
            || now >= self.expires_at
            || self.started.elapsed() >= Duration::from_secs(self.expires_at - self.approved_at)
            || current != &self.source
            || origin != self.origin()
            || generation != self.generation()
            || upstream != self.upstream()
        {
            Err(PairingError)
        } else {
            Ok(())
        }
    }
    pub fn origin(&self) -> &str {
        &self.request.origin
    }
    pub fn peer(&self) -> &str {
        &self.request.peer
    }
    pub fn upstream(&self) -> &str {
        &self.request.upstream
    }
    pub fn generation(&self) -> &str {
        &self.generation
    }
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
    pub fn source(&self) -> &BoundAccessGrant {
        &self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> BoundAccessGrant {
        BoundAccessGrant {
            installation_id: "machine".into(),
            issuer: "https://labby.example".into(),
            subject: "subject".into(),
            principal_id: "principal".into(),
            organization_id: "org".into(),
            project_id: "project".into(),
            loadout_id: "sandbox".into(),
            loadout_generation: 1,
            assignment_generation: 1,
            catalog_generation: 1,
            route_id: "sandbox".into(),
            route_generation: 1,
            membership_epoch: 1,
            organization_policy_epoch: 1,
            project_policy_epoch: 1,
            credential_id: "source".into(),
            credential_generation: 1,
            scopes: vec!["lab".into()],
            resource: "https://labby.example/sandbox".into(),
            audience: "https://labby.example/sandbox".into(),
            expires_at: 2000,
            requires_admin: false,
            destructive: false,
        }
    }
    fn route() -> ProtectedMcpRouteConfig {
        toml::from_str(
            r#"name = "sandbox"
public_host = "labby.example"
public_path = "/sandbox"
[target]
kind = "gateway_subset"
project_id = "project"
loadout = "sandbox"
"#,
        )
        .unwrap()
    }
    fn loadout() -> GatewayLoadoutConfig {
        GatewayLoadoutConfig {
            name: "sandbox".into(),
            upstreams: vec!["msb".into()],
            expose_resources: false,
            expose_prompts: false,
            expose_skills: false,
            ..Default::default()
        }
    }
    fn request() -> PairingRequest {
        PairingRequest {
            origin: "https://depot.example".into(),
            peer: format!("nodekey:{}", "1".repeat(64)),
            upstream: "msb".into(),
        }
    }
    #[test]
    fn accepts_only_native_bound_sandbox_projection() {
        assert!(
            PendingPairing::prepare(request(), source(), "machine", &route(), &loadout(), 1000)
                .is_ok()
        );
    }
    #[test]
    fn rejects_machine_admin_expiry_and_projection_widening() {
        let mut s = source();
        s.installation_id = "elsewhere".into();
        assert!(
            PendingPairing::prepare(request(), s, "machine", &route(), &loadout(), 1000).is_err()
        );
        let mut s = source();
        s.requires_admin = true;
        assert!(
            PendingPairing::prepare(request(), s, "machine", &route(), &loadout(), 1000).is_err()
        );
        assert!(
            PendingPairing::prepare(request(), source(), "machine", &route(), &loadout(), 2000)
                .is_err()
        );
        for l in [
            GatewayLoadoutConfig {
                services: vec!["fs".into()],
                ..loadout()
            },
            GatewayLoadoutConfig {
                upstreams: vec!["msb".into(), "shell".into()],
                ..loadout()
            },
            GatewayLoadoutConfig {
                expose_code_mode: true,
                ..loadout()
            },
        ] {
            assert!(
                PendingPairing::prepare(request(), source(), "machine", &route(), &l, 1000)
                    .is_err()
            );
        }
    }
    #[test]
    fn approval_is_one_use_and_expires_after_five_minutes() {
        let mut p =
            PendingPairing::prepare(request(), source(), "machine", &route(), &loadout(), 1000)
                .unwrap();
        let nonce = p.nonce().unwrap();
        assert!(p.approve(&nonce, &request(), &source(), 1299).is_ok());
        assert!(p.approve(&nonce, &request(), &source(), 1299).is_err());
        let mut p =
            PendingPairing::prepare(request(), source(), "machine", &route(), &loadout(), 1000)
                .unwrap();
        assert!(
            p.approve(&p.nonce().unwrap(), &request(), &source(), 1300)
                .is_err()
        );
    }
    #[test]
    fn approval_rechecks_nonce_peer_origin_upstream_and_native_generation() {
        for case in 0..6 {
            let mut p =
                PendingPairing::prepare(request(), source(), "machine", &route(), &loadout(), 1000)
                    .unwrap();
            let mut nonce = p.nonce().unwrap();
            let mut r = request();
            let mut current = source();
            match case {
                0 => nonce[0] ^= 1,
                1 => r.peer = format!("nodekey:{}", "2".repeat(64)),
                2 => r.origin = "https://other.example".into(),
                3 => r.upstream = "shell".into(),
                4 => current.credential_generation += 1,
                _ => current.project_policy_epoch += 1,
            }
            assert!(p.approve(&nonce, &r, &current, 1100).is_err());
        }
    }
    #[test]
    fn rejects_invalid_pairing_identity_before_approval() {
        for case in 0..5 {
            let mut r = request();
            match case {
                0 => r.origin = "http://depot.example".into(),
                1 => r.origin = "https://depot.example/path".into(),
                2 => r.origin = "https://user@depot.example".into(),
                3 => r.peer = format!("nodekey:{}", "0".repeat(64)),
                _ => r.upstream = "shell".into(),
            }
            assert!(
                PendingPairing::prepare(r, source(), "machine", &route(), &loadout(), 1000)
                    .is_err()
            );
        }
    }
    #[test]
    fn session_checks_live_authority_on_every_request_and_caps_lifetime() {
        let mut p =
            PendingPairing::prepare(request(), source(), "machine", &route(), &loadout(), 1000)
                .unwrap();
        let a = p
            .approve(&p.nonce().unwrap(), &request(), &source(), 1000)
            .unwrap();
        assert_eq!(a.expires_at(), 1900);
        assert!(
            a.authorize(a.origin(), a.generation(), a.upstream(), &source(), 1100)
                .is_ok()
        );
        assert!(
            a.authorize(a.origin(), a.generation(), a.upstream(), &source(), 1900)
                .is_err()
        );
        assert!(
            a.authorize(
                "https://other.example",
                a.generation(),
                a.upstream(),
                &source(),
                1100
            )
            .is_err()
        );
        assert!(
            a.authorize(a.origin(), "stale", a.upstream(), &source(), 1100)
                .is_err()
        );
        assert!(
            a.authorize(a.origin(), a.generation(), "shell", &source(), 1100)
                .is_err()
        );
        let mut current = source();
        current.credential_generation += 1;
        assert!(
            a.authorize(a.origin(), a.generation(), a.upstream(), &current, 1100)
                .is_err()
        );
    }
}
