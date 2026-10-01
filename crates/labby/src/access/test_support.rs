//! Protected store-backed caller fixtures shared by product domain tests.

use crate::access::{AccessStore, BootstrapOwnerInput};
use labby_auth::{Authenticator, VerifiedIdentity};

pub(crate) fn secure_tempdir() -> tempfile::TempDir {
    let base = std::env::current_dir().expect("resolve the test working directory");
    let directory = tempfile::Builder::new()
        .prefix("labby-access-test-")
        .tempdir_in(base)
        .expect("create an access fixture outside the symlinked macOS temporary directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("restrict access fixture permissions");
    }
    #[cfg(windows)]
    {
        // The journal contract checks both ownership and a private directory
        // ACL. A workspace TempDir otherwise inherits the runner's broad ACL.
        let handle = labby_winjob::fs::open_directory(directory.path())
            .expect("open the owned access fixture directory");
        labby_winjob::fs::harden_private_directory_dacl(directory.path(), &handle)
            .expect("restrict access fixture ACL");
        labby_winjob::fs::set_created_owner(directory.path(), &handle, true)
            .expect("set the access fixture owner to the current SID");
        labby_winjob::fs::verify_private_directory_dacl(&handle)
            .expect("verify the access fixture directory is private");
    }
    directory
}

pub(crate) fn browser(subject: &str) -> VerifiedIdentity {
    VerifiedIdentity::external(
        Authenticator::BrowserSession,
        "https://accounts.google.com",
        subject,
    )
    .unwrap()
}

/// Open a fresh access store with one bootstrapped owner (a platform admin
/// whose personal owner scope is `personal/bootstrap-owner`).
pub(crate) async fn fixture() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
    // The harness digest is derived from the provider URL at create time;
    // no test connects to this address unless it drives execution.
    crate::dispatch::phoenix_openai::install_test_base_url("http://127.0.0.1:9/v1");
    let directory = secure_tempdir();
    let store = AccessStore::open(directory.path().join("access.db"))
        .await
        .unwrap();
    let owner = browser("owner-subject");
    store
        .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
        .await
        .unwrap();
    (directory, store, owner)
}
