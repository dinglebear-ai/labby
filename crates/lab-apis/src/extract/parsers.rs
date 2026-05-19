//! Per-app config parsers.
//!
//! Each parser knows two things:
//! 1. **Where its config file lives** under an appdata root
//!    (`radarr/config.xml`, `prowlarr/config.xml`, ...).
//! 2. **How to extract creds** from that file's bytes.
//!
//! Parsers are sync and dyn-compatible — `ExtractClient` holds a
//! `Vec<Box<dyn Parser>>` and walks them in order against every appdata
//! subdirectory it finds. Adding a new parser is one file in this directory
//! plus one line in `all()`.

use std::path::{Path, PathBuf};

use super::error::ExtractError;
use super::types::ServiceCreds;

pub(super) mod ini;
/// Linkding config parser.
pub mod linkding;
/// Overseerr config parser.
pub mod overseerr;
/// Plex Media Server config parser.
pub mod plex;
/// Prowlarr parser (same XML shape as Radarr).
pub mod prowlarr;
/// qBittorrent WebUI config parser.
pub mod qbittorrent;
/// Concrete parsers for the Servarr stack.
pub mod radarr;
/// SABnzbd config parser.
pub mod sabnzbd;
/// Sonarr parser (same XML shape as Radarr).
pub mod sonarr;
/// Tautulli config parser.
pub mod tautulli;

/// One per-app parser. All methods are sync; I/O is the transport layer's job.
pub trait Parser: Send + Sync {
    /// Lowercase service name — `"radarr"`, `"sonarr"`, etc.
    fn name(&self) -> &'static str;

    /// Path to this app's config file relative to an appdata root.
    /// e.g. `radarr/config.xml`.
    fn config_path(&self, appdata_root: &Path) -> PathBuf;

    /// Parse the file's raw bytes into a `ServiceCreds`.
    ///
    /// # Errors
    /// Returns `ExtractError::Parse` if the file is malformed or required
    /// fields are missing.
    fn parse(&self, contents: &[u8]) -> Result<ServiceCreds, ExtractError>;
}

/// All built-in parsers, in scan order. Adding a new app means adding one
/// line here and one new file under `parsers/`.
#[must_use]
pub fn all() -> Vec<Box<dyn Parser>> {
    vec![
        Box::new(radarr::RadarrParser),
        Box::new(sonarr::SonarrParser),
        Box::new(prowlarr::ProwlarrParser),
        Box::new(sabnzbd::SabnzbdParser),
        Box::new(qbittorrent::QbittorrentParser),
        Box::new(plex::PlexParser),
        Box::new(tautulli::TautulliParser),
        Box::new(overseerr::OverseerrParser),
        Box::new(linkding::LinkdingParser),
    ]
}
