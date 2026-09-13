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
                    tracing::warn!(error = %error, "Depot authority heartbeat failed");
                }
            }
            if last_retention.elapsed() >= RETENTION_INTERVAL {
                last_retention = Instant::now();
                if let Err(error) = sender
                    .store
                    .retain_authority_projection(
                        unix_now().saturating_sub(OUTBOX_RETENTION_SECONDS),
                    )
                    .await
                {
                    tracing::warn!(error = %error, "Depot authority outbox retention failed");
                }
            }
        }
    })))
}

async fn synchronize_baseline(
    sender: &AuthorityProjectionSender,
    now: i64,
) -> Result<(), ProjectionSendError> {
    let organizations = sender.store.authority_organizations().await.map_err(|_| {
        tracing::warn!("could not enumerate authority organizations");
        ProjectionSendError::Store
    })?;
    if organizations.is_empty() {
        return Err(ProjectionSendError::Store);
    }

    for organization in organizations {
        let (ack, cutoff) = sender.send_current_snapshot(&organization, now).await?;
        sender
            .commit_snapshot_ack(&organization, &ack, cutoff.unwrap_or_default(), now)
            .await?;
    }
    sender.refresh_posture(false).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectionWork {
    Baseline,
    Deltas,
}

#[derive(Clone, Copy, Debug)]
struct ProjectionLoopState {
    needs_snapshot: bool,
}

impl ProjectionLoopState {
    const fn needs_snapshot() -> Self {
        Self {
            needs_snapshot: true,
        }
    }

    const fn next_work(self) -> ProjectionWork {
        if self.needs_snapshot {
            ProjectionWork::Baseline
        } else {
            ProjectionWork::Deltas
        }
    }

    const fn baseline_finished(&mut self, accepted_and_checkpointed: bool) {
        self.needs_snapshot = !accepted_and_checkpointed;
    }
}

async fn decode_authority_response<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ProjectionSendError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUTHORITY_RESPONSE_BYTES as u64)
    {
        return Err(ProjectionSendError::InvalidResponse);
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(MAX_AUTHORITY_RESPONSE_BYTES);
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProjectionSendError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_AUTHORITY_RESPONSE_BYTES {
            return Err(ProjectionSendError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| ProjectionSendError::InvalidResponse)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ProjectionReadiness {
    pub(crate) ready: bool,
    #[serde(default)]
    pub(crate) accepting: Option<bool>,
    #[serde(default)]
    pub(crate) organizations: BTreeMap<String, ProjectionWatermark>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ProjectionWatermark {
    pub(crate) highest_contiguous_sequence: u64,
    pub(crate) last_envelope_digest: Option<String>,
}

#[derive(Deserialize)]
struct ProjectionResponse {
    ack: AuthorityProjectionAck,
}

#[derive(Deserialize)]
struct SpooledAuthorityRecord {
    resource_type: String,
    resource_id: String,
    value: Value,
}

fn read_spooled_chunk(
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
) -> Result<Vec<AuthorityProjectionRecord>, ProjectionSendError> {
    let mut chunk = Vec::with_capacity(MAX_AUTHORITY_RECORDS_PER_BATCH);
    while chunk.len() < MAX_AUTHORITY_RECORDS_PER_BATCH {
        let Some(line) = lines.next() else { break };
        let record: SpooledAuthorityRecord =
            serde_json::from_str(&line.map_err(|_| ProjectionSendError::Store)?)
                .map_err(|_| ProjectionSendError::Store)?;
        chunk.push(AuthorityProjectionRecord {
            sequence: 0,
            resource_type: record.resource_type,
            resource_id: record.resource_id,
            operation: RecordOperation::Upsert(record.value),
        });
    }
    Ok(chunk)
}

/// Sign an envelope under the shared canonical profile: the signature covers
/// the canonical JSON of every field except `signature`; the payload digest
/// covers the canonical JSON of `records`. Both refuse non-integer numbers.
fn sign_envelope(
    installation_id: &str,
    key_id: &str,
    key: &SigningKey,
    input: EnvelopeInput<'_>,
) -> Result<AuthorityProjectionEnvelope, ProjectionSendError> {
    let payload_digest =
        canonical_json::digest(&input.records).map_err(|_| ProjectionSendError::Configuration)?;
    let mut envelope = AuthorityProjectionEnvelope {
        schema_version: AUTHORITY_PROJECTION_SCHEMA_VERSION,
        installation_id: installation_id.into(),
        organization_id: input.organization_id.into(),
        sequence_start: input.sequence_start,
        sequence_end: input.sequence_end,
        kind: input.kind,
        generated_at: input.generated_at,
        previous_digest: input.previous_digest,
        payload_digest: payload_digest.as_str().to_owned(),
        key_id: key_id.into(),
        records: input.records,
        signature: String::new(),
    };
    let signing_bytes = signing_bytes(&envelope)?;
    envelope.signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(key.sign(&signing_bytes).to_bytes());
    Ok(envelope)
}

/// Canonical JSON of every envelope field except `signature`.
fn signing_bytes(envelope: &AuthorityProjectionEnvelope) -> Result<Vec<u8>, ProjectionSendError> {
    let mut value =
        serde_json::to_value(envelope).map_err(|_| ProjectionSendError::Configuration)?;
    value
        .as_object_mut()
        .ok_or(ProjectionSendError::Configuration)?
        .remove("signature");
    canonical_json::canonical_value(&value).map_err(|_| ProjectionSendError::Configuration)
}

fn snapshot_id(organization_id: &str, now: i64) -> String {
    let mut digest = Sha256::new();
    digest.update(organization_id.as_bytes());
    digest.update(now.to_be_bytes());
    format!("snapshot-{}", hex::encode(digest.finalize()))
}

fn rfc3339_timestamp(epoch_seconds: i64) -> Result<String, ProjectionSendError> {
    if epoch_seconds < 0 {
        return Err(ProjectionSendError::Configuration);
    }
    let days = epoch_seconds.div_euclid(86_400);
    let seconds = epoch_seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    if !(0..=9_999).contains(&year) {
        return Err(ProjectionSendError::Configuration);
    }
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{AccessStore, BootstrapOwnerInput};
    use axum::extract::State;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use ed25519_dalek::{Verifier as _, VerifyingKey};
    use labby_auth::{Authenticator, VerifiedIdentity};
    use serde_json::json;

    #[tokio::test]
    async fn authority_projection_response_bodies_are_bounded() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let router = Router::new().route(
            "/",
            get(|| async {
                Json(json!({
                    "ready": true,
                    "organizations": {},
                    "padding": "x".repeat(MAX_AUTHORITY_RESPONSE_BYTES + 1),
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let response = Client::new()
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        assert!(matches!(
            decode_authority_response::<ProjectionReadiness>(response).await,
            Err(ProjectionSendError::InvalidResponse)
        ));
    }

    #[test]
    fn managed_projection_health_fails_closed_until_synchronized() {
        set_projection_health(true, false, Some("projection lag is nonzero"));
        assert_eq!(
            managed_projection_readiness_pending().as_deref(),
            Some("projection lag is nonzero")
        );
        // Marking ready without a Depot acknowledgement stays stale/pending.
        mark_projection_ready("key");
        record_delivery_posture(&[], false);
        assert!(managed_projection_readiness_pending().is_some());
        assert!(!projection_readiness().unwrap().ready);
        record_projection_ack(&AuthorityProjectionAck {
            organization_id: "org".into(),
            highest_contiguous_sequence: 3,
            last_envelope_digest: format!("sha256:{}", "aa".repeat(32)),
            snapshot_digest: None,
        });
        assert!(managed_projection_readiness_pending().is_none());
        assert!(projection_readiness().unwrap().ready);
        set_projection_health(false, true, None);
    }

    #[test]
    fn transient_baseline_rejection_cannot_fall_through_to_empty_deltas() {
        let mut state = ProjectionLoopState::needs_snapshot();
        assert_eq!(state.next_work(), ProjectionWork::Baseline);
        state.baseline_finished(false);
        assert_eq!(state.next_work(), ProjectionWork::Baseline);
        state.baseline_finished(true);
        assert_eq!(state.next_work(), ProjectionWork::Deltas);
    }

    #[test]
    fn epoch_seconds_are_encoded_as_depot_parseable_rfc3339() {
        assert_eq!(rfc3339_timestamp(0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339_timestamp(951_782_400).unwrap(),
            "2000-02-29T00:00:00Z"
        );
        assert!(rfc3339_timestamp(-1).is_err());
    }

    #[test]
    fn spooled_snapshots_never_materialize_more_than_one_envelope() {
        let count = MAX_AUTHORITY_RECORDS_PER_BATCH * 8 + 1;
        let mut lines = (0..count).map(|sequence| {
            Ok::<_, std::io::Error>(
                json!({
                    "resource_type":"principal",
                    "resource_id":format!("principal-{sequence}"),
                    "value":{"status":"active"}
                })
                .to_string(),
            )
        });
        let mut total = 0;
        loop {
            let chunk = read_spooled_chunk(&mut lines).unwrap();
            assert!(chunk.len() <= MAX_AUTHORITY_RECORDS_PER_BATCH);
            total += chunk.len();
            if chunk.is_empty() {
                break;
            }
        }
        assert_eq!(total, count);
    }

    fn record(sequence: u64, value: Value) -> AuthorityProjectionRecord {
        AuthorityProjectionRecord {
            sequence,
            resource_type: "team".into(),
            resource_id: "t1".into(),
            operation: RecordOperation::Upsert(value),
        }
    }

    #[test]
    fn canonical_signing_is_stable_verifiable_and_refuses_floats() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let build = || {
            sign_envelope(
                "install",
                "key",
                &key,
                EnvelopeInput {
                    organization_id: "org",
                    kind: ProjectionKind::Delta,
                    sequence_start: 1,
                    sequence_end: 1,
                    generated_at: "2026-09-05T00:00:00Z".into(),
                    previous_digest: None,
                    records: vec![record(1, json!({"z":1,"a":2}))],
                },
            )
            .unwrap()
        };
        let one = build();
        assert_eq!(one, build());
        assert!(Sha256Digest::is_canonical(&one.payload_digest));
        assert!(!one.signature.contains('='));
        // Depot verifies the signature over canonical JSON of the envelope
        // without `signature`; the payload digest covers canonical `records`.
        let verifying = VerifyingKey::from(&key);
        let signature = ed25519_dalek::Signature::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&one.signature)
                .unwrap(),
        )
        .unwrap();
        verifying
            .verify(&signing_bytes(&one).unwrap(), &signature)
            .unwrap();
        assert_eq!(
            one.payload_digest,
            canonical_json::digest(&one.records).unwrap().as_str()
        );
        let float = sign_envelope(
            "install",
            "key",
            &key,
            EnvelopeInput {
                organization_id: "org",
                kind: ProjectionKind::Delta,
                sequence_start: 1,
                sequence_end: 1,
                generated_at: "2026-09-05T00:00:00Z".into(),
                previous_digest: None,
                records: vec![record(1, json!({"ratio": 0.5}))],
            },
        );
        assert!(matches!(float, Err(ProjectionSendError::Configuration)));
    }

    #[test]
    fn heartbeat_envelopes_pin_the_watermark_and_carry_no_records() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let digest = format!("sha256:{}", "bb".repeat(32));
        let heartbeat = sign_envelope(
            "install",
            "key",
            &key,
            EnvelopeInput {
                organization_id: "org",
                kind: ProjectionKind::Heartbeat,
                sequence_start: 9,
                sequence_end: 9,
                generated_at: "2026-09-05T00:00:00Z".into(),
                previous_digest: Some(digest.clone()),
                records: Vec::new(),
            },
        )
        .unwrap();
        assert!(heartbeat.within_bounds());
        let encoded = serde_json::to_value(&heartbeat).unwrap();
        assert_eq!(encoded["kind"], "heartbeat");
        assert_eq!(encoded["records"], json!([]));
        assert_eq!(encoded["sequence_start"], 9);
        assert_eq!(encoded["sequence_end"], 9);
        assert_eq!(encoded["previous_digest"], digest);
        assert_eq!(
            encoded["payload_digest"],
            canonical_json::digest(&json!([])).unwrap().as_str()
        );
    }

    #[test]
    fn projection_response_requires_the_ack_wrapper() {
        let wrapped = json!({"ack": {
            "organization_id": "org-1",
            "highest_contiguous_sequence": 4,
            "last_envelope_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "snapshot_digest": null
        }});
        let response: ProjectionResponse = serde_json::from_value(wrapped).unwrap();
        assert_eq!(response.ack.organization_id, "org-1");
        assert!(
            serde_json::from_value::<ProjectionResponse>(json!({
                "organization_id": "org-1",
                "highest_contiguous_sequence": 4,
                "last_envelope_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "snapshot_digest": null
            }))
            .is_err()
        );
    }

    #[test]
    fn projection_endpoint_requires_https_or_loopback_without_secrets_in_the_url() {
        assert!(validate_projection_base_url("https://depot.example.test").is_ok());
        assert!(validate_projection_base_url("http://127.0.0.1:4100").is_ok());
        assert!(validate_projection_base_url("http://[::1]:4100").is_ok());
        assert!(validate_projection_base_url("http://localhost:4100").is_ok());
        assert!(validate_projection_base_url("http://depot.example.test").is_err());
        assert!(validate_projection_base_url("http://10.0.0.5").is_err());
        assert!(validate_projection_base_url("https://user:pw@depot.example.test").is_err());
        assert!(validate_projection_base_url("https://depot.example.test/?x=1").is_err());
        assert!(validate_projection_base_url("http://127.0.0.1/#frag").is_err());
        assert!(validate_projection_base_url("ftp://127.0.0.1").is_err());
    }

    // ---- In-process Depot authority inbox stub ----------------------------

    #[derive(Default)]
    struct StubState {
        verifying: Option<VerifyingKey>,
        watermarks: BTreeMap<String, (u64, Option<String>)>,
        received: Vec<Value>,
        /// Respond to the next envelope for this Organization with a status.
        fail_next: BTreeMap<String, u16>,
        /// Replace the acknowledged sequence in the next ack.
        ack_override: Option<u64>,
    }

    type Shared = Arc<Mutex<StubState>>;

    async fn readiness(State(state): State<Shared>) -> Json<Value> {
        let state = state.lock().unwrap();
        let organizations = state
            .watermarks
            .iter()
            .map(|(id, (sequence, digest))| {
                (
                    id.clone(),
                    json!({"highest_contiguous_sequence": sequence, "last_envelope_digest": digest}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        Json(json!({"ready": true, "accepting": true, "organizations": organizations}))
    }

    async fn inbox(
        State(state): State<Shared>,
        Json(envelope): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let mut state = state.lock().unwrap();
        let organization = envelope["organization_id"].as_str().unwrap().to_owned();
        if let Some(status) = state.fail_next.remove(&organization) {
            return (
                StatusCode::from_u16(status).unwrap(),
                Json(json!({"ok": false})),
            );
        }
        // Verify exactly what Depot verifies: signature over canonical JSON of
        // every field except `signature`, and the payload digest of records.
        let mut unsigned = envelope.clone();
        let signature = unsigned
            .as_object_mut()
            .unwrap()
            .remove("signature")
            .unwrap();
        let signing_input = canonical_json::canonical_value(&unsigned).unwrap();
        let signature = ed25519_dalek::Signature::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(signature.as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        state
            .verifying
            .as_ref()
            .unwrap()
            .verify(&signing_input, &signature)
            .expect("stub verifies the producer signature");
        assert_eq!(
            envelope["payload_digest"],
            canonical_json::digest(&envelope["records"])
                .unwrap()
                .as_str()
        );
        let (watermark, digest) = state
            .watermarks
            .get(&organization)
            .cloned()
            .unwrap_or((0, None));
        let start = envelope["sequence_start"].as_u64().unwrap();
        let end = envelope["sequence_end"].as_u64().unwrap();
        let previous = envelope["previous_digest"].as_str().map(str::to_owned);
        let new_watermark = match envelope["kind"].as_str().unwrap() {
            "heartbeat" => {
                assert_eq!(envelope["records"].as_array().unwrap().len(), 0);
                if start != watermark || end != watermark || previous != digest {
                    return (
                        StatusCode::CONFLICT,
                        Json(json!({"ok": false, "error": "chain_mismatch"})),
                    );
                }
                watermark
            }
            "snapshot" => {
                assert_eq!(start, watermark + 1);
                assert_eq!(
                    envelope["records"].as_array().unwrap().len() as u64,
                    end - start + 1
                );
                if previous != digest {
                    return (
                        StatusCode::CONFLICT,
                        Json(json!({"ok": false, "error": "chain_mismatch"})),
                    );
                }
                end
            }
            other => panic!("unexpected kind {other}"),
        };
        let envelope_digest = canonical_json::digest(&envelope)
            .unwrap()
            .as_str()
            .to_owned();
        state.watermarks.insert(
            organization.clone(),
            (new_watermark, Some(envelope_digest.clone())),
        );
        state.received.push(envelope);
        let acknowledged = state.ack_override.take().unwrap_or(new_watermark);
        (
            StatusCode::OK,
            Json(json!({"ok": true, "ack": {
                "organization_id": organization,
                "highest_contiguous_sequence": acknowledged,
                "last_envelope_digest": envelope_digest,
                "snapshot_digest": null
            }})),
        )
    }

    async fn stub() -> (Shared, Url) {
        drop(rustls::crypto::ring::default_provider().install_default());
        let state: Shared = Arc::new(Mutex::new(StubState::default()));
        let router = Router::new()
            .route("/api/authority/readiness", get(readiness))
            .route("/api/authority/projection", post(inbox))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (state, Url::parse(&format!("http://{address}")).unwrap())
    }

    async fn fixture() -> (
        tempfile::TempDir,
        Shared,
        AuthorityProjectionSender,
        AccessStore,
    ) {
        let directory = crate::access::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(identity, "Local", "Default").unwrap())
            .await
            .unwrap();
        let (state, base) = stub().await;
        let seed = [11_u8; 32];
        state.lock().unwrap().verifying = Some(VerifyingKey::from(&SigningKey::from_bytes(&seed)));
        let sender = AuthorityProjectionSender::new(
            base,
            "bearer-secret",
            "install-1",
            "key-1",
            seed,
            store.clone(),
        )
        .unwrap();
        (directory, state, sender, store)
    }

    async fn add_organization(store: &AccessStore, organization_id: &str) {
        store
            .execute_test_statement(Box::leak(
                format!(
                    "INSERT INTO organizations VALUES('{organization_id}','Second','active',0,1,1);
                     INSERT INTO principals VALUES('{organization_id}-owner','{organization_id}','user','active',NULL,5,5);
                     INSERT INTO access_audit VALUES('{organization_id}-audit',5,NULL,'{organization_id}-owner','{organization_id}',NULL,'access.team.create','team','t','allow','test',0,'{{}}');"
                )
                .into_boxed_str(),
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn accepted_snapshot_advances_the_persisted_watermark_and_marks_rows_sent() {
        let (_directory, state, sender, store) = fixture().await;
        let now = 1_000;
        let sent = sender.send_once(now).await.unwrap();
        assert_eq!(
            sent, 3,
            "bootstrap enqueued three rows, coalesced into one snapshot"
        );
        let received = state.lock().unwrap().received.clone();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0]["kind"], "snapshot");
        assert_eq!(received[0]["snapshot_complete"], true);
        assert_eq!(received[0]["snapshot_base_sequence"], 0);
        assert_eq!(received[0]["sequence_start"], 1);
        let acknowledged = store
            .acknowledged_authority_projection("bootstrap-local".into())
            .await
            .unwrap();
        assert_eq!(acknowledged.sequence, 3);
        assert_eq!(
            acknowledged.digest,
            state.lock().unwrap().watermarks["bootstrap-local"].1
        );
        let status = store.authority_delivery_status().await.unwrap();
        assert_eq!(
            (status[0].pending, status[0].inflight, status[0].failed),
            (0, 0, 0)
        );
        // Duplicate send after a crash is a no-op: nothing is pending and no
        // envelope is re-posted.
        assert_eq!(sender.send_once(now + 1).await.unwrap(), 0);
        assert_eq!(state.lock().unwrap().received.len(), 1);
        assert_eq!(
            store
                .claim_authority_projection_batch(now + 100_000, 256)
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn server_failure_leaves_rows_claimable_and_does_not_starve_other_organizations() {
        let (_directory, state, sender, store) = fixture().await;
        add_organization(&store, "second-org").await;
        state
            .lock()
            .unwrap()
            .fail_next
            .insert("bootstrap-local".into(), 503);
        let now = 1_000;
        let error = sender.send_once(now).await.unwrap_err();
        assert!(matches!(error, ProjectionSendError::Rejected));
        // The second Organization was still delivered in the same pass.
        assert_eq!(
            store
                .acknowledged_authority_projection("second-org".into())
                .await
                .unwrap()
                .sequence,
            1
        );
        let status = store.authority_delivery_status().await.unwrap();
        let first = status
            .iter()
            .find(|organization| organization.organization_id == "bootstrap-local")
            .unwrap();
        assert_eq!(first.acknowledged, 0);
        assert_eq!(
            first.pending, 3,
            "failed rows are released for backoff, not lost"
        );
        assert_eq!(first.failed, 0);
        // After the backoff window the rows are claimable and deliver.
        assert_eq!(sender.send_once(now + 3_600).await.unwrap(), 3);
        assert_eq!(
            store
                .acknowledged_authority_projection("bootstrap-local".into())
                .await
                .unwrap()
                .sequence,
            3
        );
    }

    #[tokio::test]
    async fn wrong_sequence_acknowledgement_is_rejected_and_rows_are_released() {
        let (_directory, state, sender, store) = fixture().await;
        state.lock().unwrap().ack_override = Some(42);
        let error = sender.send_once(1_000).await.unwrap_err();
        assert!(matches!(error, ProjectionSendError::InvalidResponse));
        let acknowledged = store
            .acknowledged_authority_projection("bootstrap-local".into())
            .await
            .unwrap();
        assert_eq!(acknowledged.sequence, 0);
        assert!(acknowledged.digest.is_none());
        let status = store.authority_delivery_status().await.unwrap();
        assert_eq!(status[0].pending, 3);
    }

    #[tokio::test]
    async fn heartbeats_follow_the_cadence_and_only_after_synchronization() {
        let (_directory, state, sender, store) = fixture().await;
        // Never synchronized: no heartbeat is sent (Depot would reject it).
        assert!(sender.send_heartbeats(1_000).await.unwrap().is_empty());
        assert_eq!(state.lock().unwrap().received.len(), 0);

        sender.send_once(1_000).await.unwrap();
        // Freshly acknowledged: not due yet.
        assert!(sender.send_heartbeats(1_001).await.unwrap().is_empty());
        // Simulate the interval elapsing.
        sender.last_ack.lock().unwrap().insert(
            "bootstrap-local".into(),
            Instant::now()
                .checked_sub(Duration::from_secs(
                    AUTHORITY_HEARTBEAT_INTERVAL_SECONDS + 1,
                ))
                .unwrap(),
        );
        let beaten = sender.send_heartbeats(1_100).await.unwrap();
        assert_eq!(beaten, vec!["bootstrap-local".to_owned()]);
        let received = state.lock().unwrap().received.clone();
        assert_eq!(received.len(), 2);
        let heartbeat = &received[1];
        assert_eq!(heartbeat["kind"], "heartbeat");
        assert_eq!(heartbeat["records"], json!([]));
        // Depot's watermark counts projected records (seven in the bootstrap
        // snapshot), distinct from the three local outbox rows it covered.
        assert_eq!(heartbeat["sequence_start"], received[0]["sequence_end"]);
        assert_eq!(heartbeat["sequence_end"], received[0]["sequence_end"]);
        assert_eq!(
            heartbeat["previous_digest"],
            canonical_json::digest(&received[0]).unwrap().as_str()
        );
        // The heartbeat refreshed freshness without touching the outbox.
        assert_eq!(
            store
                .acknowledged_authority_projection("bootstrap-local".into())
                .await
                .unwrap()
                .sequence,
            3
        );
        assert!(!sender.heartbeat_due("bootstrap-local"));
        // Readiness advanced only through real acknowledgements.
        assert!(projection_readiness().is_none_or(|readiness| !readiness.stale));

        // Depot lost the chain (restored from an older backup): the heartbeat
        // is refused as a chain divergence and a gap is recorded.
        state.lock().unwrap().watermarks.insert(
            "bootstrap-local".into(),
            (1, Some(format!("sha256:{}", "cd".repeat(32)))),
        );
        sender.last_ack.lock().unwrap().insert(
            "bootstrap-local".into(),
            Instant::now()
                .checked_sub(Duration::from_secs(
                    AUTHORITY_HEARTBEAT_INTERVAL_SECONDS + 1,
                ))
                .unwrap(),
        );
        assert!(matches!(
            sender.send_heartbeats(1_200).await,
            Err(ProjectionSendError::ChainDiverged)
        ));
    }
}
