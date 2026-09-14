//! Ephemeral browser sessions minted from Labby's static bearer credential.
//!
//! The long-lived bearer is presented only to the exchange endpoint. Successful
//! exchange returns an HttpOnly cookie plus a per-session CSRF token; subsequent
//! browser requests never need the bearer again. Sessions are process-local by
//! design, so a daemon restart invalidates every derived browser session.

use std::time::Duration;

use dashmap::DashMap;

use crate::error::AuthError;
use crate::types::BrowserSessionRow;
use crate::util::{now_unix, random_token};

pub const STATIC_BROWSER_SESSION_COOKIE_NAME: &str = "labby_bearer_session";
const DEFAULT_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const MAX_SESSIONS: usize = 256;

#[derive(Debug)]
pub struct StaticBrowserSessionState {
    sessions: DashMap<String, BrowserSessionRow>,
    cookie_name: String,
    ttl: Duration,
    secure_cookie: bool,
}

impl StaticBrowserSessionState {
    #[must_use]
    pub fn new(secure_cookie: bool) -> Self {
        Self {
            sessions: DashMap::new(),
            cookie_name: STATIC_BROWSER_SESSION_COOKIE_NAME.to_string(),
            ttl: DEFAULT_TTL,
            secure_cookie,
        }
    }

    #[must_use]
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    pub fn create(&self) -> Result<BrowserSessionRow, AuthError> {
        self.remove_expired();
        while self.sessions.len() >= MAX_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .min_by_key(|entry| entry.created_at)
                .map(|entry| entry.key().clone());
            let Some(oldest) = oldest else { break };
            self.sessions.remove(&oldest);
        }

        let created_at = now_unix();
        let ttl = i64::try_from(self.ttl.as_secs())
            .map_err(|_| AuthError::Config("static browser session TTL is too large".into()))?;
        let row = BrowserSessionRow {
            session_id: random_token(32)?,
            subject: "static-bearer".to_string(),
            email: None,
            csrf_token: random_token(24)?,
            created_at,
            expires_at: created_at.saturating_add(ttl),
            project_binding: None,
        };
        self.sessions.insert(row.session_id.clone(), row.clone());
        Ok(row)
    }

    #[must_use]
    pub fn find(&self, session_id: &str) -> Option<BrowserSessionRow> {
        let row = self.sessions.get(session_id)?.clone();
        if row.expires_at <= now_unix() {
            drop(row);
            self.sessions.remove(session_id);
            return None;
        }
        Some(row)
    }

    pub fn revoke(&self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    #[must_use]
    pub fn set_cookie(&self, session_id: &str) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!(
            "{}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
            self.cookie_name,
            self.ttl.as_secs(),
            secure,
        )
    }

    #[must_use]
    pub fn clear_cookie(&self) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!(
            "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT{}",
            self.cookie_name, secure,
        )
    }

    fn remove_expired(&self) {
        let now = now_unix();
        self.sessions.retain(|_, row| row.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_distinct_http_only_sessions() {
        let state = StaticBrowserSessionState::new(false);
        let first = state.create().unwrap();
        let second = state.create().unwrap();
        assert_ne!(first.session_id, second.session_id);
        assert_ne!(first.csrf_token, second.csrf_token);
        assert!(state.set_cookie(&first.session_id).contains("HttpOnly"));
        assert!(!state.set_cookie(&first.session_id).contains("; Secure"));
        assert_eq!(
            state.find(&first.session_id).unwrap().subject,
            "static-bearer"
        );
    }

    #[test]
    fn secure_cookie_mode_sets_secure_attribute() {
        let state = StaticBrowserSessionState::new(true);
        let session = state.create().unwrap();
        assert!(state.set_cookie(&session.session_id).contains("; Secure"));
        state.revoke(&session.session_id);
        assert!(state.find(&session.session_id).is_none());
    }
}
