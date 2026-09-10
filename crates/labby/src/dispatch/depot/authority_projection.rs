//! Asynchronous, bounded Labby authority projection into Depot.
//!
//! v1 delivery is **snapshot-only**: every pending outbox row for an
//! Organization is coalesced into one complete signed snapshot of current
//! authority state. Per-event deltas are never replayed. While an Organization
//! is idle the producer sends a signed heartbeat at least every
//! [`AUTHORITY_HEARTBEAT_INTERVAL_SECONDS`] so Depot can tell a quiet authority
//! from a dead one. Readiness advances only on real Depot acknowledgements
//! (snapshot or heartbeat), never on a local timer.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use labby_primitives::authority_projection::{
    AUTHORITY_HEARTBEAT_INTERVAL_SECONDS, AUTHORITY_PROJECTION_SCHEMA_VERSION,
    AuthorityProjectionAck, AuthorityProjectionEnvelope, AuthorityProjectionRecord,
    MAX_AUTHORITY_ENVELOPE_BYTES, MAX_AUTHORITY_RECORDS_PER_BATCH, ProjectionKind, RecordOperation,
};
use labby_primitives::canonical_json;
use labby_primitives::digest::Sha256Digest;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::access::AccessStore;

const SEND_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_AUTHORITY_RESPONSE_BYTES: usize = 1024 * 1024;
const LOOP_INTERVAL: Duration = Duration::from_secs(5);
/// Readiness is stale when no Depot acknowledgement (snapshot or heartbeat)
/// arrived within two heartbeat intervals.
const STALE_AFTER: Duration = Duration::from_secs(2 * AUTHORITY_HEARTBEAT_INTERVAL_SECONDS);
/// Delivered rows covered by the acknowledged watermark are pruned after this.
const OUTBOX_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;
const RETENTION_INTERVAL: Duration = Duration::from_mins(5);

#[derive(Clone, Debug, Default)]
struct ProjectionHealth {
    managed: bool,
    ready: bool,
    pending: Option<String>,
    /// Instant of the last real Depot acknowledgement.
    last_success: Option<Instant>,
    lag: usize,
    gap: bool,
    failed: usize,
    key_generation: Option<String>,
    watermark: Option<u64>,
}

#[cfg_attr(feature = "api-docs", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, serde::Serialize)]
pub struct ManagedProjectionReadiness {
    pub managed: bool,
    pub ready: bool,
    pub stale: bool,
    /// Outbox rows not yet covered by an acknowledged snapshot.
    pub lag: usize,
    /// Depot's chain disagreed with the persisted watermark; a full snapshot
    /// resynchronization is pending.
    pub gap: bool,
    /// Outbox rows parked after exhausting delivery attempts.
    pub failed: usize,
    pub watermark: Option<u64>,
    pub key_generation: Option<String>,
    pub pending: Option<String>,
}

fn projection_health() -> &'static Mutex<ProjectionHealth> {
    static HEALTH: OnceLock<Mutex<ProjectionHealth>> = OnceLock::new();
    HEALTH.get_or_init(|| Mutex::new(ProjectionHealth::default()))
}

fn set_projection_health(managed: bool, ready: bool, pending: Option<&str>) {
    if let Ok(mut health) = projection_health().lock() {
        let last_success = health.last_success;
        *health = ProjectionHealth {
            managed,
            ready,
            pending: pending.map(str::to_owned),
            // A health transition never fabricates freshness: only a Depot
            // acknowledgement moves `last_success`.
            last_success: if managed { last_success } else { None },
            lag: usize::from(!ready),
            gap: false,
            failed: 0,
            key_generation: None,
            watermark: None,
        };
    }
}

fn is_stale(health: &ProjectionHealth) -> bool {
    health
        .last_success
        .is_none_or(|success| success.elapsed() > STALE_AFTER)
}

pub(crate) fn projection_readiness() -> Option<ManagedProjectionReadiness> {
    projection_health().lock().ok().and_then(|health| {
        health.managed.then(|| {
            let stale = is_stale(&health);
            ManagedProjectionReadiness {
                managed: true,
                ready: health.ready
                    && !stale
                    && health.lag == 0
                    && !health.gap
                    && health.failed == 0,
                stale,
                lag: health.lag,
                gap: health.gap,
                failed: health.failed,
                watermark: health.watermark,
                key_generation: health.key_generation.clone(),
                pending: health.pending.clone(),
            }
        })
    })
}

/// `true` when Depot-delegated mutations may proceed: managed mode with the
/// kill switch off, protocol v1, and a projection that is synchronized, fresh,
/// gap-free, and has no parked deliveries. Adapters call this at the mutation
/// boundary; it never falls back to standalone semantics.
pub(crate) fn managed_mutations_ready(
    preferences: &crate::config::depot::DepotPreferences,
) -> bool {
    let ready = projection_readiness().is_some_and(|readiness| readiness.ready);
    preferences.managed_mutations_ready(ready, 1)
}

/// Returns a non-sensitive readiness reason while managed authority is stale.
pub(crate) fn managed_projection_readiness_pending() -> Option<String> {
    projection_health().lock().ok().and_then(|health| {
        let stale = is_stale(&health);
        (health.managed
            && (!health.ready || stale || health.lag > 0 || health.gap || health.failed > 0))
            .then(|| {
                health.pending.clone().unwrap_or_else(|| {
                    if health.failed > 0 {
                        "Depot authority projection has parked deliveries".to_owned()
                    } else if health.gap {
                        "Depot authority projection chain requires resynchronization".to_owned()
                    } else if stale {
                        "Depot authority projection acknowledgement is stale".to_owned()
                    } else {
                        "Depot authority projection is not ready".to_owned()
                    }
                })
            })
    })
}

/// Record a real Depot acknowledgement. This is the only path that refreshes
/// `last_success`.
fn record_projection_ack(ack: &AuthorityProjectionAck) {
    if let Ok(mut health) = projection_health().lock() {
        health.last_success = Some(Instant::now());
        health.watermark = Some(
            health
                .watermark
                .map_or(ack.highest_contiguous_sequence, |current| {
                    current.max(ack.highest_contiguous_sequence)
                }),
        );
    }
}

/// Mark the producer synchronized. Freshness is untouched: it comes from
/// [`record_projection_ack`].
fn mark_projection_ready(key_id: &str) {
    if let Ok(mut health) = projection_health().lock() {
        health.managed = true;
        health.ready = true;
        health.pending = None;
        health.key_generation = Some(format!(
            "sha256:{}",
            hex::encode(Sha256::digest(key_id.as_bytes()))
        ));
    }
}

fn record_delivery_posture(status: &[crate::access::OrganizationDelivery], gap: bool) {
    if let Ok(mut health) = projection_health().lock() {
        health.lag = status
            .iter()
            .map(|organization| organization.pending + organization.inflight)
            .sum();
        health.failed = status.iter().map(|organization| organization.failed).sum();
        health.gap = gap;
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectionSendError {
    #[error("authority projection configuration is invalid")]
    Configuration,
    #[error("authority projection store is unavailable")]
    Store,
    #[error("authority projection transport is unavailable")]
    Transport,
    #[error("authority projection was rejected")]
    Rejected,
    #[error("authority projection response is invalid")]
    InvalidResponse,
    #[error("authority projection chain diverged from Depot; snapshot resynchronization required")]
    ChainDiverged,
}

/// Endpoint policy for the Depot authority inbox: HTTPS to a public host (the
/// repository's SSRF vocabulary) or plain HTTP to a loopback address for a
/// co-located pair. Userinfo, query, and fragment are never accepted.
pub(crate) fn validate_projection_base_url(raw: &str) -> Result<Url, ProjectionSendError> {
    if let Ok(url) = labby_primitives::ssrf::parse_validated_https_url(raw) {
        return Ok(url);
    }
    let url = Url::parse(raw).map_err(|_| ProjectionSendError::Configuration)?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if matches!(url.scheme(), "http" | "https")
        && loopback
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
    {
        Ok(url)
    } else {
        Err(ProjectionSendError::Configuration)
    }
}

#[derive(Clone)]
pub(crate) struct AuthorityProjectionSender {
    http: Client,
    endpoint: Url,
    readiness_endpoint: Url,
    bearer: Arc<str>,
    installation_id: Arc<str>,
    key_id: Arc<str>,
    signing_key: Arc<SigningKey>,
    store: AccessStore,
    /// Instant of the last acknowledgement per Organization; drives heartbeats.
    last_ack: Arc<Mutex<BTreeMap<String, Instant>>>,
}

impl std::fmt::Debug for AuthorityProjectionSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorityProjectionSender")
            .field("endpoint", &self.endpoint)
            .field("installation_id", &self.installation_id)
            .field("key_id", &self.key_id)
            .field("credentials", &"<redacted>")
            .finish_non_exhaustive()
    }
}

struct EnvelopeInput<'a> {
    organization_id: &'a str,
    kind: ProjectionKind,
    sequence_start: u64,
    sequence_end: u64,
    generated_at: String,
    previous_digest: Option<String>,
    records: Vec<AuthorityProjectionRecord>,
}

impl AuthorityProjectionSender {
    pub(crate) fn new(
        base_url: Url,
        bearer: impl Into<Arc<str>>,
        installation_id: impl Into<Arc<str>>,
        key_id: impl Into<Arc<str>>,
        secret_key: [u8; 32],
        store: AccessStore,
    ) -> Result<Self, ProjectionSendError> {
        let secret_key = Zeroizing::new(secret_key);
        let base_url = validate_projection_base_url(base_url.as_str())?;
        let endpoint = base_url
            .join("/api/authority/projection")
            .map_err(|_| ProjectionSendError::Configuration)?;
        let readiness_endpoint = base_url
            .join("/api/authority/readiness")
            .map_err(|_| ProjectionSendError::Configuration)?;
        let bearer = bearer.into();
        let installation_id = installation_id.into();
        let key_id = key_id.into();
        if bearer.is_empty() || installation_id.is_empty() || key_id.is_empty() {
            return Err(ProjectionSendError::Configuration);
        }
        Ok(Self {
            http: Client::builder()
                .timeout(SEND_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ProjectionSendError::Configuration)?,
            endpoint,
            readiness_endpoint,
            bearer,
            installation_id,
            key_id,
            signing_key: Arc::new(SigningKey::from_bytes(&secret_key)),
            store,
            last_ack: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    pub(crate) async fn readiness(&self) -> Result<ProjectionReadiness, ProjectionSendError> {
        let response = self
            .http
            .get(self.readiness_endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(ProjectionSendError::Rejected);
        }
        let readiness: ProjectionReadiness = decode_authority_response(response).await?;
        if !readiness.accepting.unwrap_or(readiness.ready) {
            return Err(ProjectionSendError::Rejected);
        }
        Ok(readiness)
    }

    async fn post_envelope(
        &self,
        envelope: &AuthorityProjectionEnvelope,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        if !envelope.within_bounds() {
            return Err(ProjectionSendError::Configuration);
        }
        let body = serde_json::to_vec(envelope).map_err(|_| ProjectionSendError::Configuration)?;
        if body.len() > MAX_AUTHORITY_ENVELOPE_BYTES {
            return Err(ProjectionSendError::Configuration);
        }
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::CONFLICT => return Err(ProjectionSendError::ChainDiverged),
            _ => return Err(ProjectionSendError::Rejected),
        }
        let response: ProjectionResponse = decode_authority_response(response).await?;
        if response.ack.organization_id != envelope.organization_id
            || !Sha256Digest::is_canonical(&response.ack.last_envelope_digest)
        {
            return Err(ProjectionSendError::InvalidResponse);
        }
        Ok(response.ack)
    }

    async fn send_snapshot_chunk(
        &self,
        organization_id: &str,
        mut records: Vec<AuthorityProjectionRecord>,
        now: i64,
        replace: bool,
        snapshot_id: &str,
        snapshot_complete: bool,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        if records.is_empty() || records.len() > MAX_AUTHORITY_RECORDS_PER_BATCH {
            return Err(ProjectionSendError::Configuration);
        }
        let readiness = self.readiness().await?;
        let watermark = readiness.organizations.get(organization_id);
        let base = watermark.map_or(0, |value| value.highest_contiguous_sequence);
        for (offset, record) in records.iter_mut().enumerate() {
            record.sequence = base
                .checked_add(
                    u64::try_from(offset).map_err(|_| ProjectionSendError::Configuration)? + 1,
                )
                .ok_or(ProjectionSendError::Configuration)?;
        }
        let record_count =
            u64::try_from(records.len()).map_err(|_| ProjectionSendError::Configuration)?;
        let envelope = self.sign_envelope(EnvelopeInput {
            organization_id,
            kind: ProjectionKind::Snapshot {
                snapshot_id: snapshot_id.to_owned(),
                snapshot_base_sequence: replace.then_some(base),
                snapshot_complete,
            },
            sequence_start: base + 1,
            sequence_end: base + record_count,
            generated_at: rfc3339_timestamp(now)?,
            previous_digest: watermark.and_then(|value| value.last_envelope_digest.clone()),
            records,
        })?;
        let ack = self.post_envelope(&envelope).await?;
        if ack.highest_contiguous_sequence != envelope.sequence_end {
            return Err(ProjectionSendError::InvalidResponse);
        }
        Ok(ack)
    }

    /// Builds a typed snapshot from the current AccessStore state. This is the
    /// reconnect path; it does not reuse audit fingerprints as resource IDs.
    pub(crate) async fn send_current_snapshot(
        &self,
        organization_id: &str,
        now: i64,
    ) -> Result<(AuthorityProjectionAck, Option<u64>), ProjectionSendError> {
        let checkpoint = self
            .store
            .authority_snapshot_checkpoint(organization_id.to_owned())
            .await
            .map_err(|_| ProjectionSendError::Store)?;
        if checkpoint.record_count == 0 {
            return Err(ProjectionSendError::Configuration);
        }
        let file = checkpoint
            .spool
            .reopen()
            .map_err(|_| ProjectionSendError::Store)?;
        let mut lines = BufReader::new(file).lines();
        let snapshot_id = snapshot_id(organization_id, now);
        let chunk_count = checkpoint
            .record_count
            .div_ceil(MAX_AUTHORITY_RECORDS_PER_BATCH);
        let mut ack = None;
        for index in 0..chunk_count {
            let chunk = read_spooled_chunk(&mut lines)?;
            if chunk.is_empty() {
                return Err(ProjectionSendError::Store);
            }
            ack = self
                .send_snapshot_chunk(
                    organization_id,
                    chunk,
                    now,
                    index == 0,
                    &snapshot_id,
                    index + 1 == chunk_count,
                )
                .await?
                .into();
        }
        if lines.next().is_some() {
            return Err(ProjectionSendError::Store);
        }
        Ok((
            ack.ok_or(ProjectionSendError::Store)?,
            checkpoint.outbox_cutoff,
        ))
    }

    /// Persist a snapshot acknowledgement locally and refresh readiness.
    async fn commit_snapshot_ack(
        &self,
        organization_id: &str,
        ack: &AuthorityProjectionAck,
        cutoff: u64,
        now: i64,
    ) -> Result<(), ProjectionSendError> {
        self.store
            .acknowledge_authority_projection(
                organization_id.to_owned(),
                cutoff,
                ack.last_envelope_digest.clone(),
                now,
            )
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, "could not persist Depot authority acknowledgement");
                ProjectionSendError::Store
            })?;
        record_projection_ack(ack);
        if let Ok(mut last) = self.last_ack.lock() {
            last.insert(organization_id.to_owned(), Instant::now());
        }
        Ok(())
    }

    /// Performs at most one bounded delivery pass. It is intended for a supervised
    /// background loop; authorization and mutation responses never await this method.
    ///
    /// Every Organization with pending rows gets exactly one snapshot attempt.
    /// A failing Organization is released for backoff and does not stop the
    /// others; the first error is reported after the pass completes.
    pub(crate) async fn send_once(&self, now: i64) -> Result<usize, ProjectionSendError> {
        let pending = self
            .store
            .claim_authority_projection_batch(now, MAX_AUTHORITY_RECORDS_PER_BATCH)
            .await
            .map_err(|_| ProjectionSendError::Store)?;
        let mut organizations: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for row in pending {
            organizations
                .entry(row.organization_id)
                .or_default()
                .push(row.sequence);
        }
        let mut sent = 0;
        let mut first_error = None;
        for (organization_id, sequences) in organizations {
            let through = sequences.iter().copied().max().unwrap_or_default();
            let outcome = match self.send_current_snapshot(&organization_id, now).await {
                Ok((ack, Some(cutoff))) => self
                    .commit_snapshot_ack(&organization_id, &ack, cutoff, now)
                    .await
                    .map(|()| sequences.len()),
                Ok((_, None)) => Err(ProjectionSendError::InvalidResponse),
                Err(error) => Err(error),
            };
            match outcome {
                Ok(count) => sent += count,
                Err(error) => {
                    tracing::warn!(
                        organization_id,
                        error = %error,
                        "Depot authority snapshot delivery failed; releasing for backoff"
                    );
                    self.store
                        .release_failed_authority_projection(organization_id, through, now)
                        .await
                        .map_err(|_| ProjectionSendError::Store)?;
                    first_error.get_or_insert(error);
                }
            }
        }
        self.refresh_posture(false).await?;
        match first_error {
            Some(error) => Err(error),
            None => Ok(sent),
        }
    }

    async fn refresh_posture(&self, gap: bool) -> Result<(), ProjectionSendError> {
        let status = self
            .store
            .authority_delivery_status()
            .await
            .map_err(|_| ProjectionSendError::Store)?;
        record_delivery_posture(&status, gap);
        Ok(())
    }

    fn heartbeat_due(&self, organization_id: &str) -> bool {
        self.last_ack.lock().ok().is_none_or(|last| {
            last.get(organization_id).is_none_or(|instant| {
                instant.elapsed() >= Duration::from_secs(AUTHORITY_HEARTBEAT_INTERVAL_SECONDS)
            })
        })
    }

    /// Send a heartbeat for every synchronized, idle Organization whose last
    /// acknowledgement is older than the heartbeat interval. Returns the
    /// Organizations that were heartbeated. A chain disagreement marks a gap
    /// and reports [`ProjectionSendError::ChainDiverged`] so the loop resyncs.
    pub(crate) async fn send_heartbeats(
        &self,
        now: i64,
    ) -> Result<Vec<String>, ProjectionSendError> {
        let status = self
            .store
            .authority_delivery_status()
            .await
            .map_err(|_| ProjectionSendError::Store)?;
        let mut sent = Vec::new();
        let mut gap = false;
        let mut first_error = None;
        for organization in status
            .iter()
            .filter(|organization| organization.pending + organization.inflight == 0)
        {
            let organization_id = organization.organization_id.as_str();
            let acknowledged = self
                .store
                .acknowledged_authority_projection(organization_id.to_owned())
                .await
                .map_err(|_| ProjectionSendError::Store)?;
            let Some(persisted_digest) = acknowledged.digest else {
                // Never synchronized: Depot would reject the heartbeat.
                continue;
            };
            if !self.heartbeat_due(organization_id) {
                continue;
            }
            match self
                .send_heartbeat(organization_id, &persisted_digest, now)
                .await
            {
                Ok(ack) => {
                    record_projection_ack(&ack);
                    if let Ok(mut last) = self.last_ack.lock() {
                        last.insert(organization_id.to_owned(), Instant::now());
                    }
                    sent.push(organization_id.to_owned());
                }
                Err(ProjectionSendError::ChainDiverged) => {
                    gap = true;
                    first_error.get_or_insert(ProjectionSendError::ChainDiverged);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        record_delivery_posture(&status, gap);
        match first_error {
            Some(error) => Err(error),
            None => Ok(sent),
        }
    }

    async fn send_heartbeat(
        &self,
        organization_id: &str,
        persisted_digest: &str,
        now: i64,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        let readiness = self.readiness().await?;
        let watermark = readiness
            .organizations
            .get(organization_id)
            .ok_or(ProjectionSendError::ChainDiverged)?;
        if watermark.last_envelope_digest.as_deref() != Some(persisted_digest) {
            return Err(ProjectionSendError::ChainDiverged);
        }
        let envelope = self.sign_envelope(EnvelopeInput {
            organization_id,
            kind: ProjectionKind::Heartbeat,
            sequence_start: watermark.highest_contiguous_sequence,
            sequence_end: watermark.highest_contiguous_sequence,
            generated_at: rfc3339_timestamp(now)?,
            previous_digest: Some(persisted_digest.to_owned()),
            records: Vec::new(),
        })?;
        let ack = self.post_envelope(&envelope).await?;
        if ack.highest_contiguous_sequence != watermark.highest_contiguous_sequence {
            return Err(ProjectionSendError::InvalidResponse);
        }
        Ok(ack)
    }

    fn sign_envelope(
        &self,
        input: EnvelopeInput<'_>,
    ) -> Result<AuthorityProjectionEnvelope, ProjectionSendError> {
        sign_envelope(
            self.installation_id.as_ref(),
            self.key_id.as_ref(),
            self.signing_key.as_ref(),
            input,
        )
    }
}

pub(crate) async fn start_managed_projection(
    preferences: &crate::config::depot::DepotPreferences,
) -> Result<Option<tokio::task::JoinHandle<()>>, ProjectionSendError> {
    use crate::config::depot::DepotControlMode;

    if preferences.control_mode != DepotControlMode::LabbyManaged
        || preferences.managed_authority_kill_switch
    {
        set_projection_health(false, true, None);
        return Ok(None);
    }
    set_projection_health(
        true,
        false,
        Some("Depot authority projection is initializing"),
    );
    let endpoint = preferences
        .authority_endpoint
        .as_deref()
        .ok_or(ProjectionSendError::Configuration)?;
    let bearer_env = preferences.authority_bearer_token_env();
    let signing_env = preferences.authority_signing_key_env();
    if !crate::config::depot::allowed_secret_reference(bearer_env)
        || !crate::config::depot::allowed_secret_reference(signing_env)
    {
        return Err(ProjectionSendError::Configuration);
    }
    let installation_id = preferences
        .authority_installation_id
        .as_deref()
        .ok_or(ProjectionSendError::Configuration)?;
    let key_id = preferences
        .authority_key_id
        .as_deref()
        .ok_or(ProjectionSendError::Configuration)?;
    let bearer = std::env::var(bearer_env).map_err(|_| ProjectionSendError::Configuration)?;
    let encoded_key =
        Zeroizing::new(std::env::var(signing_env).map_err(|_| ProjectionSendError::Configuration)?);
    let key_bytes = Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_key.trim())
            .map_err(|_| ProjectionSendError::Configuration)?,
    );
    let mut secret_key = Zeroizing::new([0_u8; 32]);
    if key_bytes.len() != secret_key.len() {
        return Err(ProjectionSendError::Configuration);
    }
    secret_key.copy_from_slice(&key_bytes);
    let path = crate::config::access_db_path().map_err(|_| ProjectionSendError::Configuration)?;
    let store = AccessStore::open_existing_current(path)
        .await
        .map_err(|_| ProjectionSendError::Store)?;
    let sender = AuthorityProjectionSender::new(
        validate_projection_base_url(endpoint)?,
        bearer,
        installation_id,
        key_id,
        *secret_key,
        store,
    )?;

    Ok(Some(tokio::spawn(async move {
        let mut loop_state = ProjectionLoopState::needs_snapshot();
        let mut interval = tokio::time::interval(LOOP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_retention = Instant::now();
        loop {
            interval.tick().await;
            if loop_state.next_work() == ProjectionWork::Baseline {
                match synchronize_baseline(&sender, unix_now()).await {
                    Ok(()) => {
                        loop_state.baseline_finished(true);
                        mark_projection_ready(sender.key_id.as_ref());
                    }
                    Err(error) => {
                        loop_state.baseline_finished(false);
                        set_projection_health(
                            true,
                            false,
                            Some("Depot authority projection initial synchronization failed"),
                        );
                        tracing::warn!(error = %error, "Depot authority baseline synchronization failed; full snapshot will be retried");
                    }
                }
                continue;
            }

            match sender.send_once(unix_now()).await {
                Ok(_) => mark_projection_ready(sender.key_id.as_ref()),
                Err(error) => {
                    if let Ok(mut health) = projection_health().lock() {
                        health.pending =
                            Some("Depot authority projection delivery failed".to_owned());
                    }
                    tracing::warn!(error = %error, "Depot authority projection delivery failed");
                }
            }
            match sender.send_heartbeats(unix_now()).await {
                Ok(_) => {}
                Err(ProjectionSendError::ChainDiverged) => {
                    tracing::warn!(
                        "Depot authority chain diverged; scheduling full snapshot resynchronization"
                    );
                    loop_state.baseline_finished(false);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Depot auth