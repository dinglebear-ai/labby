use std::fs::{self, FileType};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use axum::serve::{IncomingStream, Listener};
use nix::unistd::{Gid, Uid, chown};
use tokio::net::{UnixListener, UnixStream};

use crate::api::oauth::AuthContext;
use crate::config::McpPreferences;

const DEFAULT_SOCKET_MODE: u32 = 0o660;
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

pub(super) async fn bind(config: &UnixListenerConfig) -> Result<AuthorizedUnixListener> {
    if !config.abstract_socket {
        if let Some(parent) = config.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).context("create Unix socket parent directory")?;
        }
        remove_stale_socket_safely(&config.path).await?;
    }

    let listener = UnixListener::bind(&config.path).context("bind configured Unix socket")?;
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

    if config.owner_uid.is_some() || config.owner_gid.is_some() {
        chown(
            &config.path,
            config.owner_uid.map(Uid::from_raw),
            config.owner_gid.map(Gid::from_raw),
        )
        .context("set Unix socket owner/group")?;
    }
    let mode = config.mode.unwrap_or(DEFAULT_SOCKET_MODE);
    fs::set_permissions(&config.path, fs::Permissions::from_mode(mode))
        .context("set Unix socket permissions")?;

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

    use axum::serve::Listener as _;
    use tokio::time::timeout;

    use super::*;

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
    fn config_resolution_prefers_env_and_validates_abstract_constraints() {
        let preferences = McpPreferences {
            socket_path: Some(PathBuf::from("/configured.sock")),
            socket_mode: Some("0600".to_string()),
            socket_uid: Some(100),
            socket_gid: Some(200),
            peer_uid: Some(300),
            peer_gid: Some(400),
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
        assert_eq!(resolved.peer_policy.uid, Some(300));
        assert_eq!(resolved.peer_policy.gid, Some(400));

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
            .get("http://local.internal/peer")
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
        let _ = server.await;
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
        let accept =
            tokio::spawn(
                async move { timeout(Duration::from_millis(200), listener.accept()).await },
            );
        let client = UnixStream::connect(&path).await.unwrap();
        assert!(accept.await.unwrap().is_err());
        drop(client);
    }
}
