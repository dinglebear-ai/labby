#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::fs::{self, FileType};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::ffi::OsStrExt as _;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _,
};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use axum::serve::{IncomingStream, Listener};
#[cfg(all(target_os = "linux", target_env = "gnu"))]
use nix::fcntl::{AT_FDCWD, RenameFlags, renameat2};
use nix::unistd::{Gid, Uid, chown};
use tokio::net::{UnixListener, UnixStream};

use crate::api::oauth::AuthContext;
use crate::config::McpPreferences;

const DEFAULT_SOCKET_MODE: u32 = 0o660;
const CREATED_PARENT_MODE: u32 = 0o755;
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(super) struct PeerPolicy {
    pub(super) uid: Option<u32>,
    pub(super) gid: Option<u32>,
}

impl PeerPolicy {
    #[must_use]
    pub(super) fn enabled(self) -> bool {
        self.uid.is_some() || self.gid.is_some()
    }

    #[must_use]
    fn accepts(self, peer: PeerCredentials) -> bool {
        self.uid.is_none_or(|uid| uid == peer.uid) && self.gid.is_none_or(|gid| gid == peer.gid)
    }
}

#[derive(Debug, Clone)]
pub(super) struct UnixListenerConfig {
    path: PathBuf,
    mode: Option<u32>,
    owner_uid: Option<u32>,
    owner_gid: Option<u32>,
    pub(super) peer_policy: PeerPolicy,
    abstract_socket: bool,
}

impl UnixListenerConfig {
    #[must_use]
    pub(super) fn abstract_socket(&self) -> bool {
        self.abstract_socket
    }

    #[must_use]
    pub(super) fn mode(&self) -> Option<u32> {
        self.mode
    }

    #[must_use]
    pub(super) fn owner_uid(&self) -> Option<u32> {
        self.owner_uid
    }

    #[must_use]
    pub(super) fn owner_gid(&self) -> Option<u32> {
        self.owner_gid
    }
}

fn env_or_config(
    env: &impl Fn(&str) -> Option<String>,
    key: &str,
    configured: Option<String>,
) -> Option<String> {
    env(key).or(configured)
}

fn parse_id(key: &str, value: Option<String>, configured: Option<u32>) -> Result<Option<u32>> {
    match value {
        Some(value) => value.trim().parse::<u32>().map(Some).with_context(|| {
            format!("invalid {key} value '{value}'; expected an unsigned integer")
        }),
        None => Ok(configured),
    }
}

fn parse_socket_mode(value: &str) -> Result<u32> {
    let trimmed = value.trim();
    let digits = trimmed
        .strip_prefix("0o")
        .or_else(|| trimmed.strip_prefix("0O"))
        .unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|byte| matches!(byte, b'0'..=b'7')) {
        anyhow::bail!("invalid Unix socket mode '{value}'; expected octal such as 0660 or 0o660");
    }
    let mode = u32::from_str_radix(digits, 8)
        .with_context(|| format!("invalid Unix socket mode '{value}'"))?;
    if mode > 0o777 {
        anyhow::bail!("invalid Unix socket mode '{value}'; permission bits must be at most 0777");
    }
    Ok(mode)
}

pub(super) fn resolve_config(
    preferences: &McpPreferences,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<UnixListenerConfig> {
    let path = env_or_config(
        env,
        "LABBY_MCP_UNIX_SOCKET_PATH",
        preferences
            .socket_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    )
    .filter(|value| !value.trim().is_empty())
    .ok_or_else(|| {
        anyhow::anyhow!(
            "unix_socket transport requires LABBY_MCP_UNIX_SOCKET_PATH or mcp.socket_path"
        )
    })?;
    let path = PathBuf::from(path);
    let raw_path = path.as_os_str().as_bytes();
    let abstract_socket = raw_path.first() == Some(&b'@');
    if raw_path == b"@" {
        anyhow::bail!("abstract Unix socket path must include a name after '@'");
    }
    if abstract_socket && !cfg!(target_os = "linux") {
        anyhow::bail!("abstract @name Unix sockets are supported only on Linux");
    }
    if !abstract_socket && !path.is_absolute() {
        anyhow::bail!("filesystem Unix socket path must be absolute");
    }

    let configured_mode = env_or_config(
        env,
        "LABBY_MCP_UNIX_SOCKET_MODE",
        preferences.socket_mode.clone(),
    );
    let mode_explicit = configured_mode.is_some();
    let mode = match configured_mode {
        Some(value) => Some(parse_socket_mode(&value)?),
        None if abstract_socket => None,
        None => Some(DEFAULT_SOCKET_MODE),
    };
    let owner_uid = parse_id(
        "LABBY_MCP_UNIX_SOCKET_UID",
        env("LABBY_MCP_UNIX_SOCKET_UID"),
        preferences.socket_uid,
    )?;
    let owner_gid = parse_id(
        "LABBY_MCP_UNIX_SOCKET_GID",
        env("LABBY_MCP_UNIX_SOCKET_GID"),
        preferences.socket_gid,
    )?;
    if abstract_socket && (mode_explicit || owner_uid.is_some() || owner_gid.is_some()) {
        anyhow::bail!(
            "abstract Unix sockets do not have filesystem mode or ownership; remove socket_mode/socket_uid/socket_gid"
        );
    }

    let peer_policy = PeerPolicy {
        uid: parse_id(
            "LABBY_MCP_UNIX_PEER_UID",
            env("LABBY_MCP_UNIX_PEER_UID"),
            preferences.peer_uid,
        )?,
        gid: parse_id(
            "LABBY_MCP_UNIX_PEER_GID",
            env("LABBY_MCP_UNIX_PEER_GID"),
            preferences.peer_gid,
        )?,
    };
    if peer_policy.enabled() && !cfg!(target_os = "linux") {
        anyhow::bail!("Unix peer-credential authorization is currently supported only on Linux");
    }

    Ok(UnixListenerConfig {
        path,
        mode,
        owner_uid,
        owner_gid,
        peer_policy,
        abstract_socket,
    })
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) struct PeerCredentials {
    pub(super) pid: Option<i32>,
    pub(super) uid: u32,
    pub(super) gid: u32,
}

impl From<tokio::net::unix::UCred> for PeerCredentials {
    fn from(value: tokio::net::unix::UCred) -> Self {
        Self {
            pid: value.pid(),
            uid: value.uid(),
            gid: value.gid(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct UnixConnectInfo {
    pub(super) peer: Option<PeerCredentials>,
}

#[derive(Debug, Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

impl SocketIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        self.device == metadata.dev() && self.inode == metadata.ino()
    }
}

#[derive(Debug)]
struct SocketCleanup {
    path: PathBuf,
    identity: SocketIdentity,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if !metadata.file_type().is_socket() || !self.identity.matches(&metadata) {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path) {
            tracing::warn!(
                error = %error,
                socket_kind = "filesystem",
                action = "unix_socket.cleanup.failed",
                "failed to remove owned Unix socket during shutdown"
            );
        }
    }
}

fn existing_path_kind(file_type: FileType) -> &'static str {
    if file_type.is_socket() {
        "socket"
    } else if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_dir() {
        "directory"
    } else if file_type.is_file() {
        "file"
    } else {
        "special file"
    }
}

async fn remove_stale_socket_safely(path: &Path) -> Result<()> {
    let first = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("inspect configured Unix socket path"),
    };
    if !first.file_type().is_socket() {
        anyhow::bail!(
            "refusing to remove existing {} at configured Unix socket path",
            existing_path_kind(first.file_type())
        );
    }
    let identity = SocketIdentity::from_metadata(&first);

    match UnixStream::connect(path).await {
        Ok(stream) => {
            drop(stream);
            anyhow::bail!("configured Unix socket path is already serving connections");
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(error) => {
            return Err(error)
                .context("existing Unix socket could not be proven stale; refusing to remove it");
        }
    }

    let second = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("reinspect configured Unix socket path"),
    };
    if !second.file_type().is_socket() || !identity.matches(&second) {
        anyhow::bail!("configured Unix socket path changed during stale-socket verification");
    }
    fs::remove_file(path).context("remove verified stale Unix socket")?;
    Ok(())
}

#[derive(Debug)]
pub(super) struct AuthorizedUnixListener {
    listener: UnixListener,
    peer_policy: PeerPolicy,
    _cleanup: Option<SocketCleanup>,
}

impl AuthorizedUnixListener {
    fn accepted_peer(&self, stream: &UnixStream) -> Option<PeerCredentials> {
        match stream.peer_cred() {
            Ok(credentials) => Some(credentials.into()),
            Err(error) if self.peer_policy.enabled() => {
                tracing::warn!(
                    error = %error,
                    action = "unix_socket.peer_credentials.failed",
                    "rejecting Unix connection because kernel peer credentials are unavailable"
                );
                None
            }
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    action = "unix_socket.peer_credentials.unavailable",
                    "Unix peer credentials unavailable for non-peer-auth listener mode"
                );
                None
            }
        }
    }
}

impl Listener for AuthorizedUnixListener {
    type Io = UnixStream;
    type Addr = UnixConnectInfo;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _address)) => {
                    let peer = self.accepted_peer(&stream);
                    if self.peer_policy.enabled() {
                        let Some(credentials) = peer else {
                            drop(stream);
                            continue;
                        };
                        if !self.peer_policy.accepts(credentials) {
                            tracing::warn!(
                                peer_uid = credentials.uid,
                                peer_gid = credentials.gid,
                                peer_pid = ?credentials.pid,
                                action = "unix_socket.peer_credentials.rejected",
                                "rejected Unix connection with unauthorized kernel peer credentials"
                            );
                            drop(stream);
                            continue;
                        }
                        tracing::debug!(
                            peer_uid = credentials.uid,
                            peer_gid = credentials.gid,
                            peer_pid = ?credentials.pid,
                            action = "unix_socket.peer_credentials.accepted",
                            "accepted authorized Unix peer"
                        );
                    }
                    return (stream, UnixConnectInfo { peer });
                }
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        action = "unix_socket.accept.failed",
                        "Unix listener accept failed; retrying"
                    );
                    tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()?;
        Ok(UnixConnectInfo::default())
    }
}

impl Connected<IncomingStream<'_, AuthorizedUnixListener>> for UnixConnectInfo {
    fn connect_info(stream: IncomingStream<'_, AuthorizedUnixListener>) -> Self {
        stream.remote_addr().clone()
    }
}

fn validate_real_directory(metadata: &fs::Metadata, effective_uid: u32) -> Result<()> {
    if !metadata.is_dir() {
        anyhow::bail!(
            "Unix socket directory chain must contain only real directories, not symlinks or special files"
        );
    }
    let owner_uid = metadata.uid();
    if owner_uid != 0 && owner_uid != effective_uid {
        anyhow::bail!(
            "Unix socket directory chain must be owned by root or effective UID {effective_uid}"
        );
    }
    let mode = metadata.permissions().mode();
    if mode & 0o022 != 0 && mode & 0o1000 == 0 {
        anyhow::bail!(
            "Unix socket directory chain must not contain group/world-writable directories without the sticky bit"
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_system_alias_target(path: &Path) -> Option<&'static Path> {
    if path == Path::new("/var") {
        Some(Path::new("/private/var"))
    } else if path == Path::new("/tmp") {
        Some(Path::new("/private/tmp"))
    } else {
        None
    }
}

fn validate_trusted_directory(
    path: &Path,
    metadata: &fs::Metadata,
    effective_uid: u32,
) -> Result<fs::Metadata> {
    #[cfg(not(target_os = "macos"))]
    let _ = path;

    if metadata.file_type().is_symlink() {
        #[cfg(target_os = "macos")]
        {
            if metadata.uid() == 0
                && macos_system_alias_target(path).is_some_and(|expected_target| {
                    fs::read_link(path).is_ok_and(|target| {
                        let resolved = if target.is_absolute() {
                            target
                        } else {
                            path.parent()
                                .map(|parent| parent.join(&target))
                                .unwrap_or(target)
                        };
                        resolved == expected_target
                    })
                })
            {
                let target_metadata = fs::metadata(path)
                    .context("inspect macOS system Unix socket directory alias")?;
                validate_real_directory(&target_metadata, effective_uid)?;
                return Ok(target_metadata);
            }
        }
        anyhow::bail!(
            "Unix socket directory chain must contain only real directories, not symlinks or special files"
        );
    }
    validate_real_directory(metadata, effective_uid)?;
    Ok(metadata.clone())
}

fn validate_final_parent(path: &Path, metadata: &fs::Metadata, effective_uid: u32) -> Result<()> {
    let metadata = validate_trusted_directory(path, metadata, effective_uid)?;
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!(
            "Unix socket final parent directory must not be writable by group or other users"
        );
    }
    Ok(())
}

fn prepare_trusted_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("filesystem Unix socket path must have a parent directory")
        })?;
    let effective_uid = Uid::effective().as_raw();
    let mut current = PathBuf::new();

    for component in parent.components() {
        match component {
            Component::RootDir => current.push(Path::new("/")),
            Component::Normal(name) => current.push(name),
            Component::CurDir | Component::ParentDir => {
                anyhow::bail!(
                    "filesystem Unix socket path must not contain relative path components"
                );
            }
            Component::Prefix(_) => {
                anyhow::bail!("unsupported Unix socket path prefix");
            }
        }

        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                builder.mode(CREATED_PARENT_MODE);
                match builder.create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(error).context("create Unix socket parent directory");
                    }
                }
                fs::symlink_metadata(&current)
                    .context("inspect created Unix socket parent directory")?
            }
            Err(error) => {
                return Err(error).context("inspect Unix socket directory chain");
            }
        };
        validate_trusted_directory(&current, &metadata, effective_uid)?;
    }

    let metadata =
        fs::symlink_metadata(parent).context("reinspect Unix socket final parent directory")?;
    validate_final_parent(parent, &metadata, effective_uid)
}

fn publish_socket_no_replace(staging_path: &Path, path: &Path) -> Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        renameat2(
            AT_FDCWD,
            staging_path,
            AT_FDCWD,
            path,
            RenameFlags::RENAME_NOREPLACE,
        )
        .context("publish configured Unix socket without replacing an existing entry")?;
        return Ok(());
    }

    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        if fs::symlink_metadata(path).is_ok() {
            anyhow::bail!(
                "refusing to publish configured Unix socket without replacing an existing entry"
            );
        }
        fs::rename(staging_path, path).context("publish configured Unix socket")
    }
}

fn bind_filesystem_socket(
    path: &Path,
    mode: u32,
    owner_uid: Option<u32>,
    owner_gid: Option<u32>,
) -> Result<UnixListener> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("filesystem Unix socket path must have a parent directory")
        })?;

    // Bind inside a private directory so the socket is never reachable while it
    // still has the platform's bind-time permissions. Configure mode/ownership
    // there, then atomically publish the already-hardened socket into the trusted
    // parent. This avoids mutating the process-global umask in a multithreaded
    // daemon.
    let staging = tempfile::Builder::new()
        .prefix(".labby-socket-stage-")
        .tempdir_in(parent)
        .context("create private Unix socket staging directory")?;
    fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))
        .context("restrict Unix socket staging directory")?;
    let staging_path = staging.path().join("socket");
    let listener = UnixListener::bind(&staging_path).context("bind staged Unix socket")?;
    fs::set_permissions(&staging_path, fs::Permissions::from_mode(mode))
        .context("set staged Unix socket permissions")?;
    if owner_uid.is_some() || owner_gid.is_some() {
        chown(
            &staging_path,
            owner_uid.map(Uid::from_raw),
            owner_gid.map(Gid::from_raw),
        )
        .context("set staged Unix socket owner/group")?;
    }
    publish_socket_no_replace(&staging_path, path)?;
    drop(staging);
    Ok(listener)
}

#[cfg(target_os = "linux")]
fn tokio_abstract_path(path: &Path) -> Result<PathBuf> {
    let raw = path.as_os_str().as_bytes();
    let name = raw
        .strip_prefix(b"@")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("abstract Unix socket path must use non-empty @name notation")
        })?;
    let mut address = Vec::with_capacity(name.len() + 1);
    address.push(0);
    address.extend_from_slice(name);
    Ok(PathBuf::from(OsString::from_vec(address)))
}

pub(super) async fn bind(config: &UnixListenerConfig) -> Result<AuthorizedUnixListener> {
    if !config.abstract_socket {
        prepare_trusted_parent(&config.path)?;
        remove_stale_socket_safely(&config.path).await?;
    }

    let listener = if config.abstract_socket {
        #[cfg(target_os = "linux")]
        {
            UnixListener::bind(tokio_abstract_path(&config.path)?)
                .context("bind configured abstract Unix socket")?
        }
        #[cfg(not(target_os = "linux"))]
        {
            anyhow::bail!("abstract Unix sockets are supported only on Linux");
        }
    } else {
        bind_filesystem_socket(
            &config.path,
            config.mode.unwrap_or(DEFAULT_SOCKET_MODE),
            config.owner_uid,
            config.owner_gid,
        )?
    };
    if config.abstract_socket {
        return Ok(AuthorizedUnixListener {
            listener,
            peer_policy: config.peer_policy,
            _cleanup: None,
        });
    }

    let metadata = fs::symlink_metadata(&config.path).context("inspect bound Unix socket")?;
    if !metadata.file_type().is_socket() {
        anyhow::bail!("bound Unix socket path is not a socket");
    }
    let cleanup = Some(SocketCleanup {
        path: config.path.clone(),
        identity: SocketIdentity::from_metadata(&metadata),
    });

    Ok(AuthorizedUnixListener {
        listener,
        peer_policy: config.peer_policy,
        _cleanup: cleanup,
    })
}

#[must_use]
pub(super) fn loopback_connect_info() -> ConnectInfo<SocketAddr> {
    ConnectInfo(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
}

pub(super) async fn inject_peer_auth(mut request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<UnixConnectInfo>>()
        .and_then(|ConnectInfo(info)| info.peer);
    let Some(peer) = peer else {
        tracing::error!(
            action = "unix_socket.peer_auth.missing",
            "authorized Unix request is missing kernel peer credentials"
        );
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "authorized Unix peer credentials unavailable",
        )
            .into_response();
    };

    let subject = format!("unix-peer:uid={}:gid={}", peer.uid, peer.gid);
    request.extensions_mut().insert(AuthContext {
        actor_key: None,
        sub: subject,
        scopes: vec!["lab:read".to_string(), "lab:admin".to_string()],
        issuer: "unix-peer-credentials".to_string(),
        via_session: false,
        csrf_token: None,
        email: None,
    });
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use std::io::Write as _;
    #[cfg(target_os = "linux")]
    use std::sync::{Arc, Mutex};

    #[cfg(target_os = "linux")]
    use tokio::time::timeout;
    #[cfg(target_os = "linux")]
    use tracing::instrument::WithSubscriber as _;

    use super::*;

    #[cfg(target_os = "linux")]
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    #[cfg(target_os = "linux")]
    struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

    #[cfg(target_os = "linux")]
    impl io::Write for CapturedLogWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("capture log lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CapturedLogWriter(Arc::clone(&self.0))
        }
    }

    #[cfg(target_os = "linux")]
    impl CapturedLogs {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("capture log lock").clone())
                .expect("logs are UTF-8")
        }
    }

    fn config_for(path: PathBuf) -> UnixListenerConfig {
        UnixListenerConfig {
            path,
            mode: Some(0o640),
            owner_uid: None,
            owner_gid: None,
            peer_policy: PeerPolicy::default(),
            abstract_socket: false,
        }
    }

    #[test]
    fn socket_mode_parser_accepts_octal_and_rejects_unsafe_values() {
        assert_eq!(parse_socket_mode("0660").unwrap(), 0o660);
        assert_eq!(parse_socket_mode("0o640").unwrap(), 0o640);
        assert!(parse_socket_mode("680").is_err());
        assert!(parse_socket_mode("1000").is_err());
    }

    #[test]
    fn peer_id_parser_accepts_root_and_rejects_invalid_values() {
        assert_eq!(
            parse_id("LABBY_MCP_UNIX_PEER_UID", Some("0".to_string()), None).unwrap(),
            Some(0)
        );
        assert_eq!(
            parse_id("LABBY_MCP_UNIX_PEER_GID", None, Some(42)).unwrap(),
            Some(42)
        );
        for invalid in ["", "-1", "not-an-id", "4294967296"] {
            assert!(
                parse_id("LABBY_MCP_UNIX_PEER_UID", Some(invalid.to_string()), None).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn config_resolution_prefers_env_and_validates_abstract_constraints() {
        let preferences = McpPreferences {
            socket_path: Some(PathBuf::from("/configured.sock")),
            socket_mode: Some("0600".to_string()),
            socket_uid: Some(100),
            socket_gid: Some(200),
            peer_uid: cfg!(target_os = "linux").then_some(300),
            peer_gid: cfg!(target_os = "linux").then_some(400),
            ..McpPreferences::default()
        };
        let env = HashMap::from([
            (
                "LABBY_MCP_UNIX_SOCKET_PATH".to_string(),
                "/env.sock".to_string(),
            ),
            ("LABBY_MCP_UNIX_SOCKET_MODE".to_string(), "0660".to_string()),
            ("LABBY_MCP_UNIX_SOCKET_UID".to_string(), "101".to_string()),
        ]);
        let resolved = resolve_config(&preferences, &|key| env.get(key).cloned()).unwrap();
        assert_eq!(resolved.path, PathBuf::from("/env.sock"));
        assert_eq!(resolved.mode, Some(0o660));
        assert_eq!(resolved.owner_uid, Some(101));
        assert_eq!(resolved.owner_gid, Some(200));
        assert_eq!(
            resolved.peer_policy.uid,
            cfg!(target_os = "linux").then_some(300)
        );
        assert_eq!(
            resolved.peer_policy.gid,
            cfg!(target_os = "linux").then_some(400)
        );

        let relative = McpPreferences {
            socket_path: Some(PathBuf::from("relative.sock")),
            ..McpPreferences::default()
        };
        let error = resolve_config(&relative, &|_| None).unwrap_err();
        assert!(error.to_string().contains("must be absolute"));

        #[cfg(target_os = "linux")]
        {
            let abstract_preferences = McpPreferences {
                socket_path: Some(PathBuf::from("@labby-test")),
                ..McpPreferences::default()
            };
            assert!(resolve_config(&abstract_preferences, &|_| None).is_ok());

            let invalid = McpPreferences {
                socket_path: Some(PathBuf::from("@labby-test")),
                socket_mode: Some("0660".to_string()),
                ..McpPreferences::default()
            };
            assert!(resolve_config(&invalid, &|_| None).is_err());
        }
    }

    #[tokio::test]
    async fn regular_files_and_active_sockets_are_never_removed() {
        let tempdir = tempfile::tempdir().unwrap();
        let file_path = tempdir.path().join("not-a-socket");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&file_path)
            .unwrap();
        writeln!(file, "preserve me").unwrap();
        assert!(bind(&config_for(file_path.clone())).await.is_err());
        assert!(file_path.is_file());

        let active_path = tempdir.path().join("active.sock");
        let active = UnixListener::bind(&active_path).unwrap();
        assert!(bind(&config_for(active_path.clone())).await.is_err());
        assert!(
            fs::symlink_metadata(&active_path)
                .unwrap()
                .file_type()
                .is_socket()
        );
        drop(active);
    }

    #[tokio::test]
    async fn untrusted_or_symlinked_directory_chain_is_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let unsafe_parent = tempdir.path().join("unsafe");
        fs::create_dir(&unsafe_parent).unwrap();
        fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o777)).unwrap();
        let error = bind(&config_for(unsafe_parent.join("labby.sock")))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("group/world-writable"));

        let trusted_child = unsafe_parent.join("trusted-child");
        fs::create_dir(&trusted_child).unwrap();
        fs::set_permissions(&trusted_child, fs::Permissions::from_mode(0o700)).unwrap();
        let error = bind(&config_for(trusted_child.join("labby.sock")))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("group/world-writable"));

        let sticky_parent = tempdir.path().join("sticky-final");
        fs::create_dir(&sticky_parent).unwrap();
        fs::set_permissions(&sticky_parent, fs::Permissions::from_mode(0o1777)).unwrap();
        let error = bind(&config_for(sticky_parent.join("labby.sock")))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("final parent"));

        let real_parent = tempdir.path().join("real");
        fs::create_dir(&real_parent).unwrap();
        let symlink_parent = tempdir.path().join("linked");
        std::os::unix::fs::symlink(&real_parent, &symlink_parent).unwrap();
        let error = bind(&config_for(symlink_parent.join("labby.sock")))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("only real directories"));
    }

    #[tokio::test]
    async fn permissive_process_umask_never_publishes_an_overly_broad_socket() {
        const CHILD_ENV: &str = "LABBY_TEST_PERMISSIVE_UMASK_CHILD";
        const TEST_NAME: &str =
            "unix_listener::tests::permissive_process_umask_never_publishes_an_overly_broad_socket";

        if std::env::var_os(CHILD_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(CHILD_ENV, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child test failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        use nix::sys::stat::{Mode, umask};

        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("initial-mode.sock");
        prepare_trusted_parent(&path).unwrap();
        let previous = umask(Mode::empty());
        let listener_result = bind_filesystem_socket(&path, 0o600, None, None);
        umask(previous);
        let listener = listener_result.unwrap();
        let observed = fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(observed & !0o600, 0, "initial socket mode was {observed:o}");
        drop(listener);
        fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn publication_never_replaces_an_existing_entry() {
        let tempdir = tempfile::tempdir().unwrap();
        let staging_path = tempdir.path().join("staged.sock");
        let published_path = tempdir.path().join("published.sock");
        let listener = UnixListener::bind(&staging_path).unwrap();
        fs::write(&published_path, b"preserve me").unwrap();

        let error = publish_socket_no_replace(&staging_path, &published_path).unwrap_err();
        assert!(error.to_string().contains("without replacing"));
        assert_eq!(fs::read(&published_path).unwrap(), b"preserve me");
        assert!(staging_path.exists());
        drop(listener);
    }

    #[tokio::test]
    async fn missing_parent_is_created_without_group_or_world_write_access() {
        // macOS has a shorter `sun_path` limit than Linux. Keep the test root
        // short enough that the private staging directory plus nested parent
        // still fits in a filesystem socket address.
        let tempdir = tempfile::tempdir_in("/tmp").unwrap();
        let parent = tempdir.path().join("nested").join("runtime");
        let path = parent.join("labby.sock");

        let listener = bind(&config_for(path.clone())).await.unwrap();
        let mode = fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o022, 0, "created parent mode was {mode:o}");
        assert!(path.exists());
        drop(listener);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn stale_socket_is_reclaimed_and_owned_socket_is_cleaned_up() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("stale.sock");
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let listener = bind(&config_for(path.clone())).await.unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
        drop(listener);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_never_removes_a_replacement_inode() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("replace.sock");
        let listener = bind(&config_for(path.clone())).await.unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement").unwrap();
        drop(listener);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn abstract_socket_binds_connects_and_accepts() {
        let path = PathBuf::from(format!("@labby-hosted-test-{}", std::process::id()));
        let config = UnixListenerConfig {
            path: path.clone(),
            mode: None,
            owner_uid: None,
            owner_gid: None,
            peer_policy: PeerPolicy::default(),
            abstract_socket: true,
        };
        let mut listener = bind(&config).await.unwrap();
        assert_eq!(
            listener.listener.local_addr().unwrap().as_abstract_name(),
            Some(path.as_os_str().as_bytes().strip_prefix(b"@").unwrap())
        );
        let accept = tokio::spawn(async move { listener.accept().await.1 });
        let client = UnixStream::connect(tokio_abstract_path(&path).unwrap())
            .await
            .unwrap();
        let info = timeout(Duration::from_secs(1), accept)
            .await
            .unwrap()
            .unwrap();

        assert!(info.peer.is_some());
        drop(client);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn peer_policy_accepts_matching_kernel_credentials() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("peer-ok.sock");
        let current = PeerPolicy {
            uid: Some(Uid::effective().as_raw()),
            gid: Some(Gid::effective().as_raw()),
        };
        let mut config = config_for(path.clone());
        config.peer_policy = current;
        let mut listener = bind(&config).await.unwrap();
        let accept = tokio::spawn(async move {
            let (_stream, info) = listener.accept().await;
            info
        });
        let client = UnixStream::connect(&path).await.unwrap();
        let info = timeout(Duration::from_secs(1), accept)
            .await
            .unwrap()
            .unwrap();
        let peer = info.peer.unwrap();
        assert!(current.accepts(peer));
        drop(client);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn axum_request_receives_kernel_derived_peer_principal() {
        use axum::Extension;
        use axum::Router;
        use axum::middleware;
        use axum::routing::get;

        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("peer-principal.sock");
        let mut config = config_for(path.clone());
        config.peer_policy = PeerPolicy {
            uid: Some(Uid::effective().as_raw()),
            gid: Some(Gid::effective().as_raw()),
        };
        let listener = bind(&config).await.unwrap();
        let router = Router::new()
            .route(
                "/peer",
                get(|Extension(context): Extension<AuthContext>| async move { context.sub }),
            )
            .layer(Extension(loopback_connect_info()))
            .layer(middleware::from_fn(inject_peer_auth));
        let service = router.into_make_service_with_connect_info::<UnixConnectInfo>();
        let server = tokio::spawn(async move { axum::serve(listener, service).await });

        drop(rustls::crypto::ring::default_provider().install_default());
        // This authority is only an HTTP Host value. The connection itself is
        // confined to the configured Unix socket, so TLS is neither used nor
        // appropriate for this transport-level test.
        let response = reqwest::Client::builder()
            .http1_only()
            .unix_socket(path.clone())
            .build()
            .unwrap()
            .get("http://localhost/peer")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let subject = response.text().await.unwrap();
        assert_eq!(
            subject,
            format!(
                "unix-peer:uid={}:gid={}",
                Uid::effective().as_raw(),
                Gid::effective().as_raw()
            )
        );

        server.abort();
        drop(server.await);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn peer_authenticated_web_session_reports_authenticated_admin() {
        use axum::middleware;

        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("peer-session.sock");
        let mut config = config_for(path.clone());
        config.peer_policy = PeerPolicy {
            uid: Some(Uid::effective().as_raw()),
            gid: Some(Gid::effective().as_raw()),
        };
        let listener = bind(&config).await.unwrap();
        let router = crate::api::router::build_router_with_external_auth(
            crate::api::state::AppState::new(),
            None,
            None,
            None,
            &[],
            true,
        )
        .layer(axum::Extension(loopback_connect_info()))
        .layer(middleware::from_fn(inject_peer_auth));
        let service = router.into_make_service_with_connect_info::<UnixConnectInfo>();
        let server = tokio::spawn(async move { axum::serve(listener, service).await });

        drop(rustls::crypto::ring::default_provider().install_default());
        let response = reqwest::Client::builder()
            .http1_only()
            .unix_socket(path.clone())
            .build()
            .unwrap()
            .get("http://localhost/auth/session")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["authenticated"], true);
        assert_eq!(body["is_admin"], true);
        assert_eq!(
            body["user"]["sub"],
            format!(
                "unix-peer:uid={}:gid={}",
                Uid::effective().as_raw(),
                Gid::effective().as_raw()
            )
        );

        server.abort();
        drop(server.await);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn peer_policy_rejects_mismatched_kernel_credentials() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("peer-denied.sock");
        let mut config = config_for(path.clone());
        config.peer_policy = PeerPolicy {
            uid: Some(Uid::effective().as_raw().wrapping_add(1)),
            gid: None,
        };
        let mut listener = bind(&config).await.unwrap();
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(logs.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let rejected = async move { timeout(Duration::from_millis(200), listener.accept()).await }
            .with_subscriber(dispatch);
        let (client, rejected) = tokio::join!(UnixStream::connect(&path), rejected);
        let client = client.unwrap();
        assert!(rejected.is_err());
        drop(client);

        let output = logs.text();
        assert!(output.contains("unix_socket.peer_credentials.rejected"));
        assert!(!output.contains(&path.to_string_lossy().to_string()));
    }
}
