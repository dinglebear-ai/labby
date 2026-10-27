use crate::BridgeError;
use sha2::{Digest, Sha256};
use std::{fmt, io::Read, net::SocketAddr, path::PathBuf};

/// Resolved configuration provided by an authorized product owner.
pub struct BridgeConfig {
    pub executable: PathBuf,
    pub state_dir: PathBuf,
    pub expected_sha256: [u8; 32],
    pub target: SocketAddr,
    pub peer: String,
    pub derp_map_url: String,
}

impl fmt::Debug for BridgeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BridgeConfig([redacted])")
    }
}

/// Verified bytes, copied into private executable custody before spawning.
/// Only Unix supervisors retain custody; other platforms can validate artifacts
/// but expose no native bridge lifecycle.
pub struct ValidatedBridgeConfig {
    #[cfg(unix)]
    pub(crate) config: BridgeConfig,
    #[cfg(unix)]
    pub(crate) bytes: Vec<u8>,
}

impl BridgeConfig {
    pub fn validate(self) -> Result<ValidatedBridgeConfig, BridgeError> {
        let ip = match self.target.ip() {
            std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(self.target.ip(), Into::into),
            ip => ip,
        };
        let peer = self
            .peer
            .strip_prefix("nodekey:")
            .ok_or(BridgeError::InvalidConfig)?;
        let url = url::Url::parse(&self.derp_map_url).map_err(|_| BridgeError::InvalidConfig)?;
        if !self.executable.is_absolute()
            || !self.state_dir.is_absolute()
            || !ip.is_loopback()
            || self.target.port() == 0
            || peer.len() != 64
            || !peer
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || peer.bytes().all(|b| b == b'0')
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(BridgeError::InvalidConfig);
        }
        let meta = std::fs::symlink_metadata(&self.executable)
            .map_err(|_| BridgeError::ArtifactUnavailable)?;
        if !meta.is_file() || meta.len() > 128 * 1024 * 1024 {
            return Err(BridgeError::InvalidConfig);
        }
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&self.executable)
                .map_err(|_| BridgeError::ArtifactUnavailable)?
        };
        #[cfg(not(unix))]
        let file =
            std::fs::File::open(&self.executable).map_err(|_| BridgeError::ArtifactUnavailable)?;
        if !file
            .metadata()
            .map_err(|_| BridgeError::ArtifactUnavailable)?
            .is_file()
        {
            return Err(BridgeError::InvalidConfig);
        }
        let mut bytes = Vec::new();
        file.take(128 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| BridgeError::ArtifactUnavailable)?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err(BridgeError::InvalidConfig);
        }
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        if hash != self.expected_sha256 {
            return Err(BridgeError::ChecksumMismatch);
        }
        Ok(ValidatedBridgeConfig {
            #[cfg(unix)]
            config: self,
            #[cfg(unix)]
            bytes,
        })
    }
}
