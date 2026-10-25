//! Bounded, subject-specific consent after an upstream scope challenge.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use super::{AuthorizationManager, OauthError, UpstreamOauthManager};
use crate::upstream::cache::OAuthLifecycleEpoch;

pub type OauthScopeChallengeHandler = Arc<dyn Fn(String) + Send + Sync>;
const MAX_SUBJECTS: usize = 256;
const MAX_SCOPES: usize = 64;
const MAX_SCOPE_BYTES: usize = 4096;
const MAX_ATTEMPTS: u8 = 3;
const CHALLENGE_TTL: Duration = Duration::from_mins(10);

#[derive(Clone)]
struct Challenge {
    scopes: Vec<String>,
    attempts: u8,
    created: Instant,
    epoch: Option<OAuthLifecycleEpoch>,
    consent: Option<(String, Vec<String>)>,
}

/// Remaining permissions that belong to one completed, current callback.
/// Only the gateway's credential writer can carry them to its replacement epoch.
pub struct ScopeChallengeTransfer {
    authority: Weak<ScopeUpgradeChallenges>,
    subject: String,
    challenge: Challenge,
    state: String,
}

impl Challenge {
    fn live(&self) -> bool {
        self.created.elapsed() < CHALLENGE_TTL
            && self
                .epoch
                .as_ref()
                .is_none_or(OAuthLifecycleEpoch::is_current)
    }
}

#[derive(Default)]
pub(super) struct ScopeUpgradeChallenges(Mutex<BTreeMap<String, Challenge>>);

fn scopes(value: &str) -> Option<Vec<String>> {
    if value.len() > MAX_SCOPE_BYTES {
        return None;
    }
    let mut values = BTreeSet::new();
    for value in value.split(' ').filter(|value| !value.is_empty()) {
        if !value.bytes().all(|byte| {
            byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
        }) {
            return None;
        }
        values.insert(value.to_string());
    }
    (!values.is_empty() && values.len() <= MAX_SCOPES).then(|| values.into_iter().collect())
}

fn auth_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// Separate challenge schemes without treating quoted commas or auth-parameter
/// whitespace as a scheme boundary. Check the complete field before selecting
/// a challenge so trailing unterminated quotes cannot be ignored.
fn authentication_challenges(header: &str) -> Option<Vec<&str>> {
    if header.len() > 8192 || header.chars().any(|ch| ch.is_control() && ch != '\t') {
        return None;
    }
    let header = header.trim_matches([' ', '\t']);
    let first_token = header.bytes().take_while(|byte| auth_token(*byte)).count();
    if first_token == 0
        || header[first_token..]
            .chars()
            .next()
            .is_some_and(|ch| ch != ' ' && ch != '\t' && ch != ',')
    {
        return None;
    }
    let mut challenges = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut fields = 1;
    for (index, ch) in header.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if ch == ',' && !quoted {
            fields += 1;
            if fields > 256 {
                return None;
            }
            let next = header[index + 1..].trim_start_matches([' ', '\t']);
            let token_end = next.bytes().take_while(|byte| auth_token(*byte)).count();
            if token_end == 0 {
                return None;
            }
            let tail = &next[token_end..];
            if !tail.trim_start_matches([' ', '\t']).starts_with('=') {
                if tail
                    .chars()
                    .next()
                    .is_some_and(|ch| ch != ' ' && ch != '\t' && ch != ',')
                {
                    return None;
                }
                challenges.push(header[start..index].trim_matches([' ', '\t']));
                if challenges.len() >= 16 {
                    return None;
                }
                start = header.len() - next.len();
            }
        }
    }
    if quoted || escaped || header.is_empty() {
        return None;
    }
    challenges.push(header[start..].trim_matches([' ', '\t']));
    Some(challenges)
}

/// Select a complete Bearer insufficient-scope challenge from a bounded field.
/// Parameters belong to their own scheme; quoted text and duplicate parameters
/// cannot inject a permission through another challenge.
pub fn required_scope_from_challenge(header: &str) -> Option<String> {
    authentication_challenges(header)?
        .into_iter()
        .find_map(required_scope_from_bearer_challenge)
}

fn required_scope_from_bearer_challenge(header: &str) -> Option<String> {
    let (scheme, mut rest) = header.trim().split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let mut parameters = BTreeMap::new();
    while !rest.trim().is_empty() {
        rest = rest.trim_start();
        let (name, remaining) = rest.split_once('=')?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return None;
        }
        rest = remaining.trim_start();
        let mut value = String::new();
        if rest.starts_with('"') {
            let mut chars = rest[1..].char_indices();
            let mut closing = None;
            while let Some((index, ch)) = chars.next() {
                match ch {
                    '"' => {
                        closing = Some(index + 2);
                        break;
                    }
                    '\\' => value.push(chars.next()?.1),
                    ch if ch.is_control() => return None,
                    ch => value.push(ch),
                }
            }
            rest = &rest[closing?..];
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_string();
            if value.is_empty() || value.contains(char::is_whitespace) {
                return None;
            }
            rest = &rest[end..];
        }
        if parameters.insert(name, value).is_some() || parameters.len() > 16 {
            return None;
        }
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        rest = rest.strip_prefix(',')?;
        if rest.trim().is_empty() {
            return None;
        }
    }
    if parameters.get("error").map(String::as_str) != Some("insufficient_scope") {
        return None;
    }
    scopes(parameters.get("scope")?).map(|values| values.join(" "))
}

impl UpstreamOauthManager {
    pub(crate) fn same_scope_challenge_authority(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scope_challenges, &other.scope_challenges)
    }

    #[cfg(test)]
    pub(crate) fn record_scope_challenge(&self, subject: &str, required: &str) {
        self.record_scope_challenge_with_epoch(subject, required, None);
    }

    pub(crate) fn record_scope_challenge_with_epoch(
        &self,
        subject: &str,
        required: &str,
        epoch: Option<OAuthLifecycleEpoch>,
    ) {
        if self.is_unattended() {
            return;
        }
        let Some(required) = scopes(required) else {
            return;
        };
        let mut entries = self
            .scope_challenges
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.live());
        if let Some(entry) = entries.get_mut(subject) {
            let combined = entry
                .scopes
                .iter()
                .chain(&required)
                .cloned()
                .collect::<BTreeSet<_>>();
            if combined.len() <= MAX_SCOPES
                && combined.iter().map(String::len).sum::<usize>() + combined.len()
                    <= MAX_SCOPE_BYTES
            {
                entry.scopes = combined.into_iter().collect();
            }
        } else if entries.len() < MAX_SUBJECTS {
            entries.insert(
                subject.to_string(),
                Challenge {
                    scopes: required,
                    attempts: 0,
                    created: Instant::now(),
                    epoch,
                    consent: None,
                },
            );
        }
    }

    /// Permissions requested by this subject's current upstream challenge.
    pub fn required_scope_upgrade(&self, subject: &str) -> Vec<String> {
        let mut entries = self
            .scope_challenges
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.live());
        entries
            .get(subject)
            .map(|entry| entry.scopes.clone())
            .unwrap_or_default()
    }

    pub(super) async fn authorization_url_with_scope_challenge(
        &self,
        manager: &AuthorizationManager,
        subject: &str,
        configured: &[&str],
    ) -> Result<String, OauthError> {
        let requested = {
            let mut entries = self
                .scope_challenges
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.retain(|_, entry| entry.live());
            match entries.get_mut(subject) {
                None => None,
                Some(entry) if entry.attempts >= MAX_ATTEMPTS => {
                    return Err(OauthError::ScopeUpgradeRequired {
                        missing_scopes: entry.scopes.clone(),
                    });
                }
                Some(entry) => {
                    entry.attempts += 1;
                    Some(entry.scopes.clone())
                }
            }
        };
        let Some(requested) = requested else {
            return manager
                .get_authorization_url(configured)
                .await
                .map_err(super::map_auth_error);
        };
        // The fresh SDK manager has no current_scopes cache. Preserve the
        // persisted grant and explicit configuration, never the AS scope catalog.
        let mut union = configured
            .iter()
            .map(|value| value.to_string())
            .chain(requested.iter().cloned())
            .collect::<BTreeSet<_>>();
        if let Some(row) = self.credential_row(subject).await? {
            let granted: Vec<String> =
                serde_json::from_str(&row.granted_scopes_json).map_err(|error| {
                    OauthError::Internal(format!("invalid stored scope metadata: {error}"))
                })?;
            union.extend(granted);
        }
        let required =
            scopes(&union.into_iter().collect::<Vec<_>>().join(" ")).ok_or_else(|| {
                OauthError::ScopeUpgradeRequired {
                    missing_scopes: requested.clone(),
                }
            })?;
        let url = manager
            .request_scope_upgrade(&required.join(" "))
            .await
            .map_err(super::map_auth_error)?;
        let csrf = super::extract_state_param(&url)
            .ok_or_else(|| OauthError::Internal("scope upgrade URL omitted state".into()))?;
        let mut entries = self
            .scope_challenges
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.get_mut(subject)
            .filter(|entry| entry.live() && entry.scopes == requested)
            .ok_or_else(|| OauthError::NeedsReauth(
                "Credentials or required permissions changed during scope authorization; start authorization again".into(),
            ))?;
        entry.consent = Some((csrf, requested));
        Ok(url)
    }

    pub(super) fn clear_completed_scope_challenge(
        &self,
        subject: &str,
        csrf: &str,
        granted: &[String],
    ) {
        let mut entries = self
            .scope_challenges
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let granted = granted.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let completed = entries.get_mut(subject).is_some_and(|entry| {
            if !entry.live()
                || !entry
                    .consent
                    .as_ref()
                    .is_some_and(|(state, _)| state == csrf)
            {
                return false;
            }
            // A successful exchange can explicitly grant fewer permissions than
            // requested. Keep every ungranted permission and its attempt budget.
            entry
                .scopes
                .retain(|scope| !granted.contains(scope.as_str()));
            entry.scopes.is_empty()
        });
        if completed {
            entries.remove(subject);
        }
    }

    /// Capture only a current challenge belonging to the callback that just
    /// completed. Call while holding the gateway credential lifecycle writer.
    pub fn scope_challenge_transfer_for_callback(
        &self,
        subject: &str,
        csrf: &str,
    ) -> Option<ScopeChallengeTransfer> {
        let entries = self
            .scope_challenges
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.get(subject).filter(|entry| {
            entry.live()
                && entry
                    .consent
                    .as_ref()
                    .is_some_and(|(state, _)| state == csrf)
        })?;
        Some(ScopeChallengeTransfer {
            authority: Arc::downgrade(&self.scope_challenges),
            subject: subject.to_string(),
            challenge: entry.clone(),
            state: csrf.to_string(),
        })
    }

    /// Preserve remaining permissions across this callback's own epoch change.
    /// The caller must retain the same lifecycle writer through capture,
    /// invalidation, and this handoff. Credential clear and revoke do not use it.
    pub fn rebind_scope_challenge_after_callback(
        &self,
        transfer: ScopeChallengeTransfer,
        epoch: OAuthLifecycleEpoch,
    ) {
        if !epoch.is_current()
            || transfer.challenge.created.elapsed() >= CHALLENGE_TTL
            || !transfer
                .authority
                .upgrade()
                .is_some_and(|authority| Arc::ptr_eq(&authority, &self.scope_challenges))
        {
            return;
        }
        let mut entries = self
            .scope_challenges
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&transfer.subject).filter(|entry| {
            entry.created == transfer.challenge.created
                && entry
                    .consent
                    .as_ref()
                    .is_some_and(|(state, _)| *state == transfer.state)
        }) {
            entry.epoch = Some(epoch);
            entry.consent = None;
            return;
        }
        // Status reads can prune the previous epoch while the credential writer
        // is replacing it. The captured callback owns a bounded snapshot so that
        // pruning cannot lose its remaining permissions or reset its budget.
        // A newer live observation has its own authority and must survive intact.
        if entries.get(&transfer.subject).is_some_and(Challenge::live) {
            return;
        }
        entries.retain(|_, entry| entry.live());
        if entries.len() >= MAX_SUBJECTS {
            return;
        }
        let mut challenge = transfer.challenge;
        challenge.epoch = Some(epoch);
        challenge.consent = None;
        entries.insert(transfer.subject, challenge);
    }
}

#[cfg(test)]
#[path = "scope_upgrade_tests.rs"]
mod tests;
