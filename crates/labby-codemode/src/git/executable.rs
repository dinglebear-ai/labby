//! Operator-selected Git executable; never resolve it relative to a workspace.

use std::path::PathBuf;

use crate::error::ToolError;

pub(super) fn git_binary() -> Result<PathBuf, ToolError> {
    resolve_git_binary(std::env::var_os("LABBY_CODE_MODE_GIT_EXE").map(PathBuf::from))
}

fn resolve_git_binary(override_exe: Option<PathBuf>) -> Result<PathBuf, ToolError> {
    let Some(path) = override_exe else {
        #[cfg(windows)]
        {
            return Ok(PathBuf::from("git.exe"));
        }
        #[cfg(not(windows))]
        {
            return Ok(PathBuf::from("/usr/bin/git"));
        }
    };
    let invalid = |reason: &str| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("LABBY_CODE_MODE_GIT_EXE {reason}"),
    };
    if !path.is_absolute() {
        return Err(invalid("must be an absolute path"));
    }
    let canonical =
        std::fs::canonicalize(&path).map_err(|_| invalid("must resolve to an executable file"))?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|_| invalid("must resolve to an executable file"))?;
    if !metadata.is_file() {
        return Err(invalid("must resolve to an executable file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o111 == 0 {
            return Err(invalid("must resolve to an executable file"));
        }
        if metadata.mode() & 0o022 != 0 {
            return Err(invalid("must not be group/world writable"));
        }
        let current_uid = nix::unistd::Uid::current().as_raw();
        if metadata.uid() != current_uid && metadata.uid() != 0 {
            return Err(invalid("must be owned by the current user or root"));
        }
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_operator_override() {
        assert!(resolve_git_binary(Some(PathBuf::from("git"))).is_err());
    }

    #[test]
    fn rejects_missing_operator_override() {
        let temp = tempfile::tempdir().unwrap();
        assert!(resolve_git_binary(Some(temp.path().join("missing"))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn validates_override_and_resolves_its_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("actual-git");
        std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = temp.path().join("git");
        symlink(&binary, &link).unwrap();
        assert_eq!(
            resolve_git_binary(Some(link)).unwrap(),
            binary.canonicalize().unwrap()
        );
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(resolve_git_binary(Some(binary.clone())).is_err());
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolve_git_binary(Some(binary)).is_err());
    }
}
