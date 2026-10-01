//! Short-lived, one-shot setup capabilities. Only token hashes live in memory.
use crate::{
    error::AuthError,
    util::{now_unix, random_token},
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Mutex};
const TTL_SECONDS: i64 = 60;
const MAX_PENDING: usize = 64;
struct Pending {
    origin: String,
    expires_at: i64,
}
#[derive(Default)]
pub(crate) struct SetupHandoffs {
    pending: Mutex<HashMap<[u8; 32], Pending>>,
}
impl std::fmt::Debug for SetupHandoffs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SetupHandoffs { <redacted> }")
    }
}
impl SetupHandoffs {
    pub(crate) fn start(&self, origin: &str) -> Result<String, AuthError> {
        self.start_at(origin, now_unix())
    }
    fn start_at(&self, origin: &str, now: i64) -> Result<String, AuthError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| AuthError::Config("setup handoff storage unavailable".into()))?;
        pending.retain(|_, row| row.expires_at > now);
        if pending.len() >= MAX_PENDING {
            return Err(AuthError::Config("setup handoff capacity reached".into()));
        }
        let token = random_token(32)?;
        pending.insert(
            Sha256::digest(token.as_bytes()).into(),
            Pending {
                origin: origin.into(),
                expires_at: now.saturating_add(TTL_SECONDS),
            },
        );
        Ok(token)
    }
    pub(crate) fn consume(&self, token: &str, origin: &str) -> bool {
        self.consume_at(token, origin, now_unix())
    }
    fn consume_at(&self, token: &str, origin: &str, now: i64) -> bool {
        if token.len() != 43
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return false;
        }
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let Some(row) = pending.get(&hash) else {
            return false;
        };
        if row.expires_at <= now {
            pending.remove(&hash);
            return false;
        }
        if row.origin != origin {
            return false;
        }
        pending.remove(&hash).is_some()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handoff_is_independent_origin_bound_expiring_and_one_shot() {
        let state = SetupHandoffs::default();
        let token = state.start_at("http://127.0.0.1:8765", 10).unwrap();
        assert_eq!(token.len(), 43);
        assert!(!state.consume_at(&token, "http://127.0.0.1:9999", 11));
        assert!(state.consume_at(&token, "http://127.0.0.1:8765", 11));
        assert!(!state.consume_at(&token, "http://127.0.0.1:8765", 11));
        let expired = state.start_at("http://127.0.0.1:8765", 10).unwrap();
        assert!(!state.consume_at(&expired, "http://127.0.0.1:8765", 70));
    }
    #[test]
    fn simultaneous_redeems_have_one_winner() {
        let state = std::sync::Arc::new(SetupHandoffs::default());
        let token = state.start_at("http://127.0.0.1:8765", 10).unwrap();
        let handles = (0..8)
            .map(|_| {
                let state = state.clone();
                let token = token.clone();
                std::thread::spawn(move || state.consume_at(&token, "http://127.0.0.1:8765", 11))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            handles
                .into_iter()
                .map(|handle| handle.join().expect("redeemer thread finished"))
                .filter(|won| *won)
                .count(),
            1
        );
    }
}
