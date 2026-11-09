//! Private installation authority for session binding and local bridge proof.
use std::{path::Path, sync::Arc};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use hmac::{Hmac, KeyInit as _, Mac as _};
use labby_auth::upstream::encryption::{EncryptionKey, load_key, open_with_aad, seal_with_aad};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use crate::dispatch::codemode_notices::NoticeError;
use crate::installation::secure_file;

pub(crate) const BRIDGE_PROOF_PATH: &str = "/v1/notifications/agent/message-bridge-proof";
pub(crate) const BRIDGE_AUTH_HEADER: &str = "labby-bridge-authorization";
pub(crate) const SESSION_KEY_HEADER: &str = "labby-session-key";
pub(crate) const BRIDGE_PROOF_MS: i64 = 5_000;
const CHALLENGE_WINDOW_MS: u64 = 30_000;
const MAX_BRIDGE_BEARER_BYTES: usize = 16 * 1024;
const BRIDGE_NONCE_BYTES: usize = 12;
const BRIDGE_TAG_BYTES: usize = 16;

#[derive(Clone)]
pub(crate) struct MessageKey(Arc<Zeroizing<[u8; 32]>>);

impl MessageKey {
    pub(crate) fn open_at(root: &Path) -> Result<Self, NoticeError> {
        let dir = root.join("agent-notifications");
        secure_file::create_private_dir(&dir).map_err(|_| NoticeError::Unavailable)?;
        let path = dir.join("messaging.key");
        let raw = match secure_file::read_private(&path) {
            Ok(raw) => Zeroizing::new(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut key = Zeroizing::new([0_u8; 32]);
                getrandom::fill(key.as_mut()).map_err(|_| NoticeError::Unavailable)?;
                match secure_file::publish_new(&path, key.as_ref()) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(NoticeError::Unavailable),
                }
                Zeroizing::new(
                    secure_file::read_private(&path).map_err(|_| NoticeError::Unavailable)?,
                )
            }
            Err(_) => return Err(NoticeError::Unavailable),
        };
        let key: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| NoticeError::Unavailable)?;
        Ok(Self(Arc::new(Zeroizing::new(key))))
    }

    #[cfg(test)]
    pub(crate) fn from_test_bytes(key: [u8; 32]) -> Self {
        Self(Arc::new(Zeroizing::new(key)))
    }

    pub(crate) fn digest(&self, domain: &str, fields: &[&[u8]]) -> String {
        hex::encode(self.mac(domain, fields))
    }

    fn mac(&self, domain: &str, fields: &[&[u8]]) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.0.as_ref().as_ref())
            .expect("32-byte installation MAC key");
        mac.update(b"labby.agent-messages.v1\0");
        for field in std::iter::once(domain.as_bytes()).chain(fields.iter().copied()) {
            mac.update(&(field.len() as u64).to_be_bytes());
            mac.update(field);
        }
        mac.finalize().into_bytes().into()
    }

    fn bridge_auth_key(&self, installation: &str) -> Result<EncryptionKey, NoticeError> {
        let derived = Zeroizing::new(self.mac("bridge-authorization", &[installation.as_bytes()]));
        let encoded = Zeroizing::new(STANDARD.encode(derived.as_slice()));
        load_key(&encoded).map_err(|_| NoticeError::Unavailable)
    }

    pub(crate) fn seal_bridge_bearer(
        &self,
        installation: &str,
        bearer: &str,
    ) -> Result<String, NoticeError> {
        if bearer.is_empty() || bearer.len() > MAX_BRIDGE_BEARER_BYTES {
            return Err(NoticeError::Identity);
        }
        let key = self.bridge_auth_key(installation)?;
        let (ciphertext, mut nonce) =
            seal_with_aad(&key, bearer.as_bytes(), BRIDGE_PROOF_PATH.as_bytes())
                .map_err(|_| NoticeError::Unavailable)?;
        nonce.extend(ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(nonce))
    }

    pub(super) fn open_bridge_bearer(
        &self,
        installation: &str,
        encoded: &str,
    ) -> Result<Zeroizing<Vec<u8>>, NoticeError> {
        const OVERHEAD: usize = BRIDGE_NONCE_BYTES + BRIDGE_TAG_BYTES;
        if encoded.len() > (MAX_BRIDGE_BEARER_BYTES + OVERHEAD).div_ceil(3) * 4 {
            return Err(NoticeError::Identity);
        }
        let envelope = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| NoticeError::Identity)?;
        if envelope.len() <= OVERHEAD || envelope.len() > MAX_BRIDGE_BEARER_BYTES + OVERHEAD {
            return Err(NoticeError::Identity);
        }
        let (nonce, ciphertext) = envelope.split_at(BRIDGE_NONCE_BYTES);
        let key = self.bridge_auth_key(installation)?;
        open_with_aad(&key, ciphertext, nonce, BRIDGE_PROOF_PATH.as_bytes())
            .map(Zeroizing::new)
            .map_err(|_| NoticeError::Identity)
    }

    pub(crate) fn static_owner(&self, bearer: &str) -> String {
        self.digest("static-owner", &[bearer.as_bytes()])
    }
}

pub(crate) fn canonical_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn check_mac(expected: &str, received: &str) -> Result<(), NoticeError> {
    if canonical_digest(received) && bool::from(expected.as_bytes().ct_eq(received.as_bytes())) {
        Ok(())
    } else {
        Err(NoticeError::Identity)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgeProofRequest {
    pub(crate) installation: String,
    pub(crate) nonce: String,
    sent_at: i64,
    mac: String,
}

impl BridgeProofRequest {
    pub(crate) fn new(
        key: &MessageKey,
        bearer: &str,
        installation: &str,
        now: i64,
    ) -> Result<Self, NoticeError> {
        if !canonical_digest(installation) {
            return Err(NoticeError::Identity);
        }
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| NoticeError::Unavailable)?;
        let mut request = Self {
            installation: installation.to_owned(),
            nonce: hex::encode(random),
            sent_at: now,
            mac: String::new(),
        };
        request.mac = request.expected_mac(key, bearer);
        Ok(request)
    }

    fn expected_mac(&self, key: &MessageKey, bearer: &str) -> String {
        key.digest(
            "bridge-request",
            &[
                self.installation.as_bytes(),
                self.nonce.as_bytes(),
                &self.sent_at.to_be_bytes(),
                bearer.as_bytes(),
            ],
        )
    }

    pub(crate) fn verify(
        &self,
        key: &MessageKey,
        bearer: &str,
        installation: &str,
        now: i64,
    ) -> Result<(), NoticeError> {
        if self.installation != installation
            || !canonical_digest(&self.nonce)
            || now.abs_diff(self.sent_at) > CHALLENGE_WINDOW_MS
        {
            return Err(NoticeError::Identity);
        }
        check_mac(&self.expected_mac(key, bearer), &self.mac)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgeProof {
    installation: String,
    nonce: String,
    pub(crate) owner_key: String,
    pub(crate) daemon_epoch: String,
    pub(crate) expires_at: i64,
    mac: String,
}

impl BridgeProof {
    pub(crate) fn issue(
        key: &MessageKey,
        request: &BridgeProofRequest,
        bearer: &str,
        owner_key: &str,
        daemon_epoch: &str,
        now: i64,
    ) -> Result<Self, NoticeError> {
        if !canonical_digest(owner_key) || !valid_epoch(daemon_epoch) {
            return Err(NoticeError::Identity);
        }
        let mut proof = Self {
            installation: request.installation.clone(),
            nonce: request.nonce.clone(),
            owner_key: owner_key.to_owned(),
            daemon_epoch: daemon_epoch.to_owned(),
            expires_at: now.saturating_add(BRIDGE_PROOF_MS),
            mac: String::new(),
        };
        proof.mac = proof.expected_mac(key, request, bearer);
        Ok(proof)
    }

    fn expected_mac(&self, key: &MessageKey, request: &BridgeProofRequest, bearer: &str) -> String {
        key.digest(
            "bridge-reply",
            &[
                self.installation.as_bytes(),
                self.nonce.as_bytes(),
                &request.sent_at.to_be_bytes(),
                self.owner_key.as_bytes(),
                self.daemon_epoch.as_bytes(),
                &self.expires_at.to_be_bytes(),
                bearer.as_bytes(),
            ],
        )
    }

    pub(crate) fn verify(
        &self,
        key: &MessageKey,
        request: &BridgeProofRequest,
        bearer: &str,
        now: i64,
    ) -> Result<(), NoticeError> {
        if self.installation != request.installation
            || self.nonce != request.nonce
            || !canonical_digest(&self.owner_key)
            || !valid_epoch(&self.daemon_epoch)
            || self.expires_at <= now
            || self.expires_at > now.saturating_add(BRIDGE_PROOF_MS)
        {
            return Err(NoticeError::Identity);
        }
        check_mac(&self.expected_mac(key, request, bearer), &self.mac)
    }
}

fn valid_epoch(value: &str) -> bool {
    value.len() == 26 && ulid::Ulid::from_string(value).is_ok()
}

#[cfg(test)]
mod tests;
