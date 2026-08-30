pub(super) fn secure_tempdir() -> tempfile::TempDir {
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
    directory
}
