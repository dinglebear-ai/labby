use std::fmt;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use tracing::warn;

#[cfg(test)]
thread_local! { static FS_FAILPOINT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
fn failpoint(point: u8) -> Result<(), AuthError> {
    if FS_FAILPOINT.with(|cell| cell.get()) == point {
        Err(AuthError::Storage(format!(
            "injected signing-key filesystem failure {point}"
        )))
    } else {
        Ok(())
    }
}
#[cfg(not(test))]
const fn failpoint(_: u8) -> Result<(), AuthError> {
    Ok(())
}

use crate::error::AuthError;
use crate::util::{
    ensure_restrictive_permissions, set_restrictive_permissions, write_secret_file_atomically,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub exp: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<usize>,
    pub iat: usize,
    pub jti: String,
    pub scope: String,
    pub azp: String,
    /// Canonical external identity-provider issuer verified before minting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_issuer: Option<String>,
    /// Stable local credential ID verified before minting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_credential_id: Option<String>,
}

#[must_use]
pub fn is_canonical_access_token_id(token_id: &str) -> bool {
    !token_id.is_empty()
        && token_id.trim() == token_id
        && token_id.len() <= 256
        && !token_id.chars().any(char::is_control)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwksDocument {
    pub keys: Vec<JwkKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwkKey {
    pub kty: String,
    #[serde(rename = "use")]
    pub use_: String,
    pub alg: String,
    pub kid: String,
    pub crv: String,
    pub x: String,
}

#[derive(Clone)]
pub struct SigningKeys {
    pub key_id: String,
    encoding_key: EncodingKey,
    decoding_keys: Vec<(String, DecodingKey)>,
    jwks: JwksDocument,
}

impl fmt::Debug for SigningKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SigningKeys")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl SigningKeys {
    pub fn load_or_create(path: &Path) -> Result<Self, AuthError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                AuthError::Storage(format!(
                    "create signing key directory `{}`: {error}",
                    parent.display()
                ))
            })?;
        }

        let existed = path.exists();
        if existed {
            ensure_restrictive_permissions(path)?;
        }

        let private_key = if existed {
            let key_bytes = std::fs::read(path).map_err(|error| {
                AuthError::Storage(format!("read signing key `{}`: {error}", path.display()))
            })?;
            match SigningKey::from_pkcs8_der(&key_bytes) {
                Ok(key) => key,
                Err(_) => {
                    // Pre-Ed25519 releases stored an RSA PKCS#8 key at this
                    // path. Quarantine any non-Ed25519 material and rotate to
                    // a constant-time signing primitive; never silently reuse
                    // the vulnerable signing path.
                    let retired =
                        path.with_extension(format!("retired-{}", crate::util::now_unix()));
                    std::fs::rename(path, &retired).map_err(|error| {
                        AuthError::Storage(format!(
                            "retire legacy signing key `{}`: {error}",
                            path.display()
                        ))
                    })?;
                    set_restrictive_permissions(&retired)?;
                    generate_signing_key(path)?
                }
            }
        } else {
            generate_signing_key(path)?
        };

        ensure_restrictive_permissions(path)?;
        let mut keys = vec![private_key];
        let retired_dir = retired_dir(path);
        if retired_dir.is_dir() {
            let now = crate::util::now_unix();
            let entries = std::fs::read_dir(&retired_dir)
                .map_err(|error| AuthError::Storage(format!("read retired signing keys: {error}")))?
                .filter_map(Result::ok)
                .collect::<Vec<_>>();
            let mut retired = Vec::new();
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(expires) = name
                    .rsplit_once('.')
                    .and_then(|(_, value)| value.parse::<i64>().ok())
                else {
                    return Err(AuthError::Storage(format!(
                        "invalid retired signing-key filename `{name}`"
                    )));
                };
                if expires > now {
                    retired.push((expires, entry.path()));
                } else {
                    std::fs::remove_file(entry.path()).map_err(|error| {
                        AuthError::Storage(format!("drain expired retired signing key: {error}"))
                    })?;
                }
            }
            retired.sort_by(|(left_expiry, left_path), (right_expiry, right_path)| {
                right_expiry
                    .cmp(left_expiry)
                    .then_with(|| right_path.cmp(left_path))
            });
            if retired.len() > 4 {
                for (_, excess_path) in retired.drain(4..) {
                    std::fs::remove_file(excess_path).map_err(|error| {
                        AuthError::Storage(format!("prune excess retired signing key: {error}"))
                    })?;
                }
            }
            for (_, retired_path) in retired {
                ensure_restrictive_permissions(&retired_path)?;
                let bytes = std::fs::read(&retired_path).map_err(|error| {
                    AuthError::Storage(format!(
                        "read retired signing key `{}`: {error}",
                        retired_path.display()
                    ))
                })?;
                keys.push(SigningKey::from_pkcs8_der(&bytes).map_err(|error| {
                    AuthError::Storage(format!(
                        "decode retired signing key `{}`: {error}",
                        retired_path.display()
                    ))
                })?);
            }
        }
        Self::from_private_keys(&keys)
    }

    pub fn issue_access_token(&self, claims: &AccessClaims) -> Result<String, AuthError> {
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.key_id.clone());
        encode(&header, &claims, &self.encoding_key)
            .map_err(|error| AuthError::Storage(format!("encode access token: {error}")))
    }

    /// Validate access token signature, algorithm, and audience.
    ///
    /// NOTE: this method does NOT enforce the `iss` claim. Callers that
    /// need RFC 7519 issuer validation MUST use
    /// [`Self::validate_access_token_with_issuer`] instead. This entry
    /// point is preserved for the lab consumer, which performs its own
    /// post-decode `iss` check. New consumers (syslog-mcp et al.) should
    /// always use the issuer-enforcing variant.
    #[deprecated(note = "Use `validate_access_token_with_issuer` for RFC 7519 §4.1.1 compliance")]
    pub fn validate_access_token(
        &self,
        token: &str,
        expected_audience: &str,
    ) -> Result<AccessClaims, AuthError> {
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_audience(&[expected_audience]);
        validation.validate_nbf = true;
        self.decode_with_ring(token, &validation).map_err(|error| {
            // `AuthError::InvalidAccessToken` renders as a single opaque
            // string, so without this the reason a token was refused —
            // expired, bad signature, wrong audience — is unrecoverable
            // from the logs. The jsonwebtoken error names the kind and
            // never embeds the token itself.
            warn!(
                kind = "auth_failed",
                reason = ?error.kind(),
                "access token rejected"
            );
            AuthError::InvalidAccessToken
        })
    }

    /// Validate signature, algorithm, audience, AND issuer in a single
    /// pass — the issuer is enforced via `Validation::set_issuer` BEFORE
    /// decode (RFC 7519 §4.1.1 compliant) rather than via a manual
    /// `claims.iss != expected` check after decode.
    pub fn validate_access_token_with_issuer(
        &self,
        token: &str,
        expected_audience: &str,
        expected_issuer: &str,
    ) -> Result<AccessClaims, AuthError> {
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_audience(&[expected_audience]);
        validation.set_issuer(&[expected_issuer]);
        validation.validate_nbf = true;
        self.decode_with_ring(token, &validation).map_err(|error| {
            warn!(
                kind = "auth_failed",
                reason = ?error.kind(),
                "access token rejected"
            );
            AuthError::InvalidAccessToken
        })
    }

    pub const fn jwks(&self) -> &JwksDocument {
        &self.jwks
    }

    fn from_private_keys(private_keys: &[SigningKey]) -> Result<Self, AuthError> {
        let private_key = private_keys
            .first()
            .ok_or_else(|| AuthError::Storage("empty signing key ring".to_string()))?;
        let private_der = private_key
            .to_pkcs8_der()
            .map_err(|error| AuthError::Storage(format!("encode signing key DER: {error}")))?;
        let public_key = private_key.verifying_key();
        let public_der = public_key
            .to_public_key_der()
            .map_err(|error| AuthError::Storage(format!("encode public key DER: {error}")))?;
        let digest = Sha256::digest(public_der.as_bytes());
        let key_id = URL_SAFE_NO_PAD.encode(&digest[..12]);

        let mut jwks_keys = Vec::with_capacity(private_keys.len());
        let mut decoding_keys = Vec::with_capacity(private_keys.len());
        for key in private_keys {
            let public = key.verifying_key();
            let public_der = public
                .to_public_key_der()
                .map_err(|error| AuthError::Storage(format!("encode public key DER: {error}")))?;
            let kid = URL_SAFE_NO_PAD.encode(&Sha256::digest(public_der.as_bytes())[..12]);
            if decoding_keys.iter().any(|(existing, _)| existing == &kid) {
                continue;
            }
            jwks_keys.push(JwkKey {
                kty: "OKP".to_string(),
                use_: "sig".to_string(),
                alg: "EdDSA".to_string(),
                kid: kid.clone(),
                crv: "Ed25519".to_string(),
                x: URL_SAFE_NO_PAD.encode(public.as_bytes()),
            });
            decoding_keys.push((kid, DecodingKey::from_ed_der(public.as_bytes())));
        }
        let jwks = JwksDocument { keys: jwks_keys };

        Ok(Self {
            key_id,
            encoding_key: EncodingKey::from_ed_der(private_der.as_bytes()),
            // jsonwebtoken's RustCrypto verifier consumes the raw 32-byte
            // Ed25519 public point here (its from_ed_der name is historical).
            decoding_keys,
            jwks,
        })
    }

    fn decode_with_ring(
        &self,
        token: &str,
        validation: &Validation,
    ) -> jsonwebtoken::errors::Result<AccessClaims> {
        let kid = decode_header(token)?.kid.ok_or_else(|| {
            jsonwebtoken::errors::Error::from(jsonwebtoken::errors::ErrorKind::InvalidToken)
        })?;
        let key = self
            .decoding_keys
            .iter()
            .find(|(candidate, _)| candidate == &kid)
            .map(|(_, key)| key)
            .ok_or_else(|| {
                jsonwebtoken::errors::Error::from(jsonwebtoken::errors::ErrorKind::InvalidSignature)
            })?;
        decode::<AccessClaims>(token, key, validation).map(|data| data.claims)
    }

    pub fn rotate(path: &Path, overlap: std::time::Duration) -> Result<Self, AuthError> {
        Self::rotate_with_minimum(path, overlap, std::time::Duration::from_hours(1))
    }

    pub fn rotate_with_minimum(
        path: &Path,
        overlap: std::time::Duration,
        maximum_access_token_ttl: std::time::Duration,
    ) -> Result<Self, AuthError> {
        validate_overlap(overlap, maximum_access_token_ttl)?;
        let current = Self::load_or_create(path)?;
        let dir = retired_dir(path);
        std::fs::create_dir_all(&dir).map_err(|e| {
            AuthError::Storage(format!("create retired signing-key directory: {e}"))
        })?;
        set_restrictive_directory_permissions(&dir)?;
        let retired_at = crate::util::now_unix();
        let expires =
            retired_at.saturating_add(i64::try_from(overlap.as_secs()).unwrap_or(i64::MAX));
        let retired = dir.join(format!("{}.{}.{}", current.key_id, retired_at, expires));
        std::fs::copy(path, &retired)
            .map_err(|e| AuthError::Storage(format!("stage retired signing key: {e}")))?;
        set_restrictive_permissions(&retired)?;
        if let Err(error) = failpoint(1) {
            drop(std::fs::remove_file(&retired));
            return Err(error);
        }
        if let Err(error) = generate_signing_key(path) {
            drop(std::fs::remove_file(&retired));
            return Err(error);
        }
        Self::load_or_create(path)
    }

    pub fn emergency_revoke(path: &Path) -> Result<Self, AuthError> {
        warn!(kind = "auth_signing_key_emergency_revoked", key_path = %path.display(), "emergency signing-key revocation invalidates all outstanding access tokens");
        let dir = retired_dir(path);
        let quarantine = path.with_extension("retired-revoking");
        if quarantine.exists() {
            return Err(AuthError::Storage(
                "stale signing-key revocation quarantine exists".to_string(),
            ));
        }
        let quarantined = dir.is_dir();
        if quarantined {
            std::fs::rename(&dir, &quarantine)
                .map_err(|e| AuthError::Storage(format!("quarantine retired signing keys: {e}")))?;
        }
        if let Err(error) = failpoint(2) {
            if quarantined {
                std::fs::rename(&quarantine, &dir).map_err(|restore| {
                    AuthError::Storage(format!("{error}; restore retired signing keys: {restore}"))
                })?;
            }
            return Err(error);
        }
        if let Err(error) = generate_signing_key(path) {
            if quarantined {
                std::fs::rename(&quarantine, &dir).map_err(|restore| {
                    AuthError::Storage(format!("{error}; restore retired signing keys: {restore}"))
                })?;
            }
            return Err(error);
        }
        if quarantined && let Err(error) = std::fs::remove_dir_all(&quarantine) {
            warn!(kind = "auth_signing_key_quarantine_cleanup_failed", %error, "active key revoked successfully; manual quarantine cleanup required");
        }
        Self::load_or_create(path)
    }

    pub fn rollback(path: &Path, overlap: std::time::Duration) -> Result<Self, AuthError> {
        Self::rollback_with_minimum(path, overlap, std::time::Duration::from_hours(1))
    }

    pub fn rollback_with_minimum(
        path: &Path,
        overlap: std::time::Duration,
        maximum_access_token_ttl: std::time::Duration,
    ) -> Result<Self, AuthError> {
        validate_overlap(overlap, maximum_access_token_ttl)?;
        // Load first: this validates the active ring and physically drains
        // expired or excess retired entries before any rollback candidate is
        // selected or buffered.
        let current = Self::load_or_create(path)?;
        let dir = retired_dir(path);
        let now = crate::util::now_unix();
        let candidate = std::fs::read_dir(&dir)
            .map_err(|e| AuthError::Storage(format!("read retired signing keys: {e}")))?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let (retired_at, expires) = retired_key_times(&name)?;
                (expires > now).then_some((retired_at, name, entry))
            })
            .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
            .map(|(_, _, entry)| entry)
            .ok_or_else(|| {
                AuthError::Config("no retired signing key is available for rollback".to_string())
            })?;
        let candidate_bytes = std::fs::read(candidate.path())
            .map_err(|e| AuthError::Storage(format!("read rollback signing key: {e}")))?;
        SigningKey::from_pkcs8_der(&candidate_bytes)
            .map_err(|error| AuthError::Storage(format!("decode rollback signing key: {error}")))?;
        let retired_at = crate::util::now_unix();
        let expires =
            retired_at.saturating_add(i64::try_from(overlap.as_secs()).unwrap_or(i64::MAX));
        let current_retired = dir.join(format!("{}.{}.{}", current.key_id, retired_at, expires));
        std::fs::copy(path, &current_retired)
            .map_err(|e| AuthError::Storage(format!("stage current key for rollback: {e}")))?;
        set_restrictive_permissions(&current_retired)?;
        if let Err(error) = failpoint(3) {
            drop(std::fs::remove_file(&current_retired));
            return Err(error);
        }
        write_secret_file_atomically(path, &candidate_bytes)?;
        if let Err(error) = std::fs::remove_file(candidate.path()) {
            warn!(kind = "auth_signing_key_rollback_cleanup_failed", %error, "rollback promoted successfully; duplicate retired file requires cleanup");
        }
        Self::load_or_create(path)
    }
}

fn validate_overlap(
    overlap: std::time::Duration,
    maximum_access_token_ttl: std::time::Duration,
) -> Result<(), AuthError> {
    if overlap < maximum_access_token_ttl {
        return Err(AuthError::Config(
            "signing-key overlap must be at least the maximum access-token lifetime".to_string(),
        ));
    }
    Ok(())
}

fn retired_key_times(name: &str) -> Option<(i64, i64)> {
    let mut parts = name.rsplit('.');
    let expires = parts.next()?.parse::<i64>().ok()?;
    let previous = parts.next()?;
    let retired_at = previous.parse::<i64>().unwrap_or_default();
    Some((retired_at, expires))
}

fn retired_dir(path: &Path) -> std::path::PathBuf {
    path.with_extension("retired")
}

fn set_restrictive_directory_permissions(path: &Path) -> Result<(), AuthError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| AuthError::Storage(format!("protect signing-key directory: {e}")))?;
    }
    Ok(())
}

fn generate_signing_key(path: &Path) -> Result<SigningKey, AuthError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| AuthError::Storage(format!("generate Ed25519 key material: {error}")))?;
    let key = SigningKey::from_bytes(&bytes);
    bytes.fill(0);
    let der = key
        .to_pkcs8_der()
        .map_err(|error| AuthError::Storage(format!("encode signing key DER: {error}")))?;
    write_secret_file_atomically(path, der.as_bytes())?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::decode_header;

    use super::{AccessClaims, SigningKeys};

    fn ring_snapshot(path: &std::path::Path) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
        let mut retired = Vec::new();
        let dir = super::retired_dir(path);
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                retired.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                ));
            }
            retired.sort_by(|a, b| a.0.cmp(&b.0));
        }
        (std::fs::read(path).unwrap(), retired)
    }

    #[test]
    fn generated_key_is_reused_on_second_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        let first = SigningKeys::load_or_create(&path).unwrap();
        let second = SigningKeys::load_or_create(&path).unwrap();
        assert_eq!(first.key_id, second.key_id);
    }

    #[test]
    #[allow(deprecated)]
    fn rotation_overlaps_then_rollback_and_emergency_revoke_are_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        let original = SigningKeys::load_or_create(&path).unwrap();
        let old_token = original.issue_access_token(&sample_claims()).unwrap();

        let rotated = SigningKeys::rotate(&path, std::time::Duration::from_hours(1)).unwrap();
        assert_ne!(rotated.key_id, original.key_id);
        assert_eq!(rotated.jwks().keys.len(), 2);
        rotated
            .validate_access_token(&old_token, "https://lab.example.com")
            .unwrap();
        let new_token = rotated.issue_access_token(&sample_claims()).unwrap();

        let rolled_back = SigningKeys::rollback(&path, std::time::Duration::from_hours(1)).unwrap();
        assert_eq!(rolled_back.key_id, original.key_id);
        rolled_back
            .validate_access_token(&new_token, "https://lab.example.com")
            .unwrap();

        let emergency = SigningKeys::emergency_revoke(&path).unwrap();
        assert_eq!(emergency.jwks().keys.len(), 1);
        assert!(
            emergency
                .validate_access_token(&old_token, "https://lab.example.com")
                .is_err()
        );
        assert!(
            emergency
                .validate_access_token(&new_token, "https://lab.example.com")
                .is_err()
        );
    }

    #[test]
    fn rotation_rejects_overlap_shorter_than_maximum_token_lifetime_before_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        let original = SigningKeys::load_or_create(&path).unwrap();
        let error = SigningKeys::rotate_with_minimum(
            &path,
            std::time::Duration::from_secs(3599),
            std::time::Duration::from_hours(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("maximum access-token lifetime"));
        assert_eq!(
            SigningKeys::load_or_create(&path).unwrap().key_id,
            original.key_id
        );
    }

    #[test]
    fn rollback_rejects_short_overlap_and_expired_candidates_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        SigningKeys::load_or_create(&path).unwrap();
        SigningKeys::rotate(&path, std::time::Duration::from_hours(1)).unwrap();
        let rotated = ring_snapshot(&path);
        let error = SigningKeys::rollback_with_minimum(
            &path,
            std::time::Duration::from_secs(3599),
            std::time::Duration::from_hours(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("maximum access-token lifetime"));
        assert_eq!(ring_snapshot(&path), rotated);

        let retired = super::retired_dir(&path);
        for entry in std::fs::read_dir(&retired).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            let kid = name.split('.').next().unwrap();
            std::fs::rename(entry.path(), retired.join(format!("{kid}.0.0"))).unwrap();
        }
        let active_before = std::fs::read(&path).unwrap();
        let error = SigningKeys::rollback(&path, std::time::Duration::from_hours(1)).unwrap_err();
        assert!(error.to_string().contains("no retired signing key"));
        assert_eq!(std::fs::read(&path).unwrap(), active_before);
        assert_eq!(std::fs::read_dir(&retired).unwrap().count(), 0);
    }

    #[test]
    fn loading_key_ring_physically_drains_expired_retired_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        let active = SigningKeys::load_or_create(&path).unwrap();
        let retired = super::retired_dir(&path);
        std::fs::create_dir_all(&retired).unwrap();
        super::set_restrictive_directory_permissions(&retired).unwrap();
        let expired = retired.join(format!("{}.0", active.key_id));
        std::fs::copy(&path, &expired).unwrap();
        crate::util::set_restrictive_permissions(&expired).unwrap();

        let loaded = SigningKeys::load_or_create(&path).unwrap();
        assert_eq!(loaded.jwks().keys.len(), 1);
        assert!(!expired.exists());
    }

    #[test]
    fn repeated_rotations_physically_bound_retired_key_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        SigningKeys::load_or_create(&path).unwrap();
        for _ in 0..8 {
            SigningKeys::rotate(&path, std::time::Duration::from_hours(1)).unwrap();
        }
        let loaded = SigningKeys::load_or_create(&path).unwrap();
        assert_eq!(loaded.jwks().keys.len(), 5);
        assert_eq!(
            std::fs::read_dir(super::retired_dir(&path))
                .unwrap()
                .count(),
            4
        );
    }

    #[test]
    fn precommit_filesystem_failures_restore_exact_key_ring() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        SigningKeys::load_or_create(&path).unwrap();
        let initial = ring_snapshot(&path);
        super::FS_FAILPOINT.with(|cell| cell.set(1));
        assert!(SigningKeys::rotate(&path, std::time::Duration::from_hours(1)).is_err());
        super::FS_FAILPOINT.with(|cell| cell.set(0));
        assert_eq!(ring_snapshot(&path), initial);

        SigningKeys::rotate(&path, std::time::Duration::from_hours(1)).unwrap();
        let rotated = ring_snapshot(&path);
        for point in [2_u8, 3_u8] {
            super::FS_FAILPOINT.with(|cell| cell.set(point));
            let result = if point == 2 {
                SigningKeys::emergency_revoke(&path)
            } else {
                SigningKeys::rollback(&path, std::time::Duration::from_hours(1))
            };
            assert!(result.is_err());
            super::FS_FAILPOINT.with(|cell| cell.set(0));
            assert_eq!(ring_snapshot(&path), rotated);
        }
    }

    #[cfg(unix)]
    #[test]
    fn signing_key_refuses_world_readable_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        std::fs::write(&path, "bad").unwrap();
        std::fs::set_permissions(&path, PermissionsExt::from_mode(0o644)).unwrap();
        let err = SigningKeys::load_or_create(&path).unwrap_err();
        assert!(err.to_string().contains("permissions"));
    }

    #[test]
    #[allow(deprecated)]
    fn minted_access_token_round_trips_and_contains_kid() {
        let signer = test_signer();
        let claims = sample_claims();
        let token = signer.issue_access_token(&claims).unwrap();
        let claims = signer
            .validate_access_token(&token, "https://lab.example.com")
            .unwrap();
        assert_eq!(claims.aud, "https://lab.example.com");
        assert!(!claims.jti.is_empty());
        assert!(decode_header(&token).unwrap().kid.is_some());
    }

    #[test]
    #[allow(deprecated)]
    fn wrong_audience_is_rejected() {
        let signer = test_signer();
        let claims = sample_claims();
        let token = signer.issue_access_token(&claims).unwrap();
        let result = signer.validate_access_token(&token, "https://other.example.com");
        assert!(
            result.is_err(),
            "token with wrong audience must be rejected"
        );
    }

    #[test]
    fn validate_with_issuer_accepts_matching_issuer() {
        let signer = test_signer();
        let claims = sample_claims();
        let token = signer.issue_access_token(&claims).unwrap();
        let decoded = signer
            .validate_access_token_with_issuer(
                &token,
                "https://lab.example.com",
                "https://lab.example.com",
            )
            .expect("token with matching issuer must validate");
        assert_eq!(decoded.iss, "https://lab.example.com");
    }

    #[test]
    fn validate_with_issuer_rejects_wrong_issuer_via_validation_struct() {
        // Locked decision: issuer enforcement uses Validation::set_issuer
        // BEFORE decode (so jsonwebtoken rejects up-front), not a manual
        // post-decode `claims.iss != expected` comparison.
        let signer = test_signer();
        let claims = sample_claims();
        let token = signer.issue_access_token(&claims).unwrap();
        let result = signer.validate_access_token_with_issuer(
            &token,
            "https://lab.example.com",
            "https://attacker.example.com",
        );
        assert!(
            result.is_err(),
            "token signed by us but with wrong expected issuer must be rejected"
        );
    }

    fn test_signer() -> SigningKeys {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        SigningKeys::load_or_create(&path).unwrap()
    }

    fn sample_claims() -> AccessClaims {
        AccessClaims {
            iss: "https://lab.example.com".to_string(),
            sub: "google-user".to_string(),
            aud: "https://lab.example.com".to_string(),
            exp: 4_102_444_800,
            nbf: None,
            iat: 1_700_000_000,
            jti: "test-jti".to_string(),
            scope: "lab".to_string(),
            azp: "client".to_string(),
            identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
            identity_credential_id: None,
        }
    }
}
