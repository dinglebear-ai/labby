use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub display: String,
}

pub fn resolve_proxy_command(
    raw: &[OsString],
    cwd: &Path,
    path_env: Option<&OsStr>,
) -> Result<ProxyCommand, ProxyCommandError> {
    let Some(target) = raw.first() else {
        return Err(ProxyCommandError::MissingCommand);
    };
    let child_args = &raw[1..];
    let target_path = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        cwd.join(target)
    };

    let (program, mut args) = if target_path.is_file() {
        resolve_file_target(target, &target_path, path_env)?
    } else if contains_path_separator(target) {
        return Err(ProxyCommandError::NotFound {
            target: target.to_string_lossy().into_owned(),
        });
    } else {
        let program =
            resolve_on_path(target, path_env).ok_or_else(|| ProxyCommandError::NotFound {
                target: target.to_string_lossy().into_owned(),
            })?;
        (program.into_os_string(), Vec::new())
    };
    args.extend(child_args.iter().cloned());
    let display = display_command(&program, &args);
    Ok(ProxyCommand {
        program,
        args,
        cwd: cwd.to_path_buf(),
        display,
    })
}

fn resolve_file_target(
    original: &OsStr,
    path: &Path,
    path_env: Option<&OsStr>,
) -> Result<(OsString, Vec<OsString>), ProxyCommandError> {
    if is_executable(path) {
        return Ok((path.as_os_str().to_os_string(), Vec::new()));
    }
    if let Some((interpreter, optional_arg)) = parse_shebang(path)? {
        let program = resolve_on_path(OsStr::new(&interpreter), path_env).ok_or_else(|| {
            ProxyCommandError::RuntimeNotFound {
                target: original.to_string_lossy().into_owned(),
                runtime: interpreter.clone(),
            }
        })?;
        let mut args = Vec::new();
        if let Some(arg) = optional_arg {
            args.push(arg.into());
        }
        args.push(path.as_os_str().to_os_string());
        return Ok((program.into_os_string(), args));
    }
    let extension = path.extension().and_then(OsStr::to_str).unwrap_or_default();
    let runtime = match extension {
        "js" | "mjs" | "cjs" => Some("node"),
        "py" => Some("python3"),
        "ts" => {
            return Err(ProxyCommandError::AmbiguousTypeScriptRuntime {
                target: original.to_string_lossy().into_owned(),
            });
        }
        _ => None,
    };
    let Some(runtime) = runtime else {
        return Err(ProxyCommandError::UnsupportedFile {
            target: original.to_string_lossy().into_owned(),
        });
    };
    let program = resolve_on_path(OsStr::new(runtime), path_env).ok_or_else(|| {
        ProxyCommandError::RuntimeNotFound {
            target: original.to_string_lossy().into_owned(),
            runtime: runtime.to_string(),
        }
    })?;
    Ok((
        program.into_os_string(),
        vec![path.as_os_str().to_os_string()],
    ))
}

fn parse_shebang(path: &Path) -> Result<Option<(String, Option<String>)>, ProxyCommandError> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path).map_err(|error| ProxyCommandError::Inspect {
        target: path.display().to_string(),
        error: error.to_string(),
    })?;
    let mut bytes = [0_u8; 512];
    let count = file
        .read(&mut bytes)
        .map_err(|error| ProxyCommandError::Inspect {
            target: path.display().to_string(),
            error: error.to_string(),
        })?;
    let content = String::from_utf8_lossy(&bytes[..count]);
    let Some(line) = content.lines().next() else {
        return Ok(None);
    };
    let Some(raw) = line.strip_prefix("#!") else {
        return Ok(None);
    };
    let mut parts = raw.split_whitespace();
    let Some(interpreter) = parts.next() else {
        return Ok(None);
    };
    if interpreter.ends_with("/env") {
        let Some(command) = parts.next() else {
            return Ok(None);
        };
        let remaining = parts.collect::<Vec<_>>();
        if remaining.len() > 1 {
            return Err(ProxyCommandError::UnsupportedShebang {
                target: path.display().to_string(),
            });
        }
        return Ok(Some((
            command.to_string(),
            remaining.first().map(ToString::to_string),
        )));
    }
    let remaining = parts.collect::<Vec<_>>();
    if remaining.len() > 1 {
        return Err(ProxyCommandError::UnsupportedShebang {
            target: path.display().to_string(),
        });
    }
    Ok(Some((
        interpreter.to_string(),
        remaining.first().map(ToString::to_string),
    )))
}

fn resolve_on_path(program: &OsStr, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    std::env::split_paths(path_env)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file() && is_executable(candidate))
}

fn contains_path_separator(value: &OsStr) -> bool {
    Path::new(value).components().count() > 1
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(not(any(unix, windows)))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn display_command(program: &OsStr, args: &[OsString]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(OsString::as_os_str))
        .map(shell_escape_for_display)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_escape_for_display(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '\\' | '.' | '_' | '-' | ':'))
    {
        value.into_owned()
    } else {
        format!("{:?}", value.as_ref())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProxyCommandError {
    #[error("proxy command is required")]
    MissingCommand,
    #[error("proxy command or file `{target}` was not found")]
    NotFound { target: String },
    #[error("cannot inspect proxy target `{target}`: {error}")]
    Inspect { target: String, error: String },
    #[error("runtime `{runtime}` required by proxy target `{target}` was not found on PATH")]
    RuntimeNotFound { target: String, runtime: String },
    #[error("cannot infer how to launch proxy target `{target}`")]
    UnsupportedFile { target: String },
    #[error(
        "TypeScript target `{target}` requires an explicit runtime such as bun, deno, or npx tsx"
    )]
    AmbiguousTypeScriptRuntime { target: String },
    #[error("proxy target `{target}` has an unsupported multi-argument shebang")]
    UnsupportedShebang { target: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn resolves_javascript_file_through_node() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let node = bin.join("node");
        fs::write(&node, "").unwrap();
        make_executable(&node);
        let script = dir.path().join("server.js");
        fs::write(&script, "console.log('server')").unwrap();
        let path = std::env::join_paths([&bin]).unwrap();

        let command = resolve_proxy_command(
            &[script.as_os_str().to_os_string(), "--child-flag".into()],
            dir.path(),
            Some(&path),
        )
        .unwrap();

        assert_eq!(command.program, node);
        assert_eq!(
            command.args,
            vec![script.into_os_string(), "--child-flag".into()]
        );
    }

    #[test]
    fn executes_executable_target_directly() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("server");
        fs::write(
            &target,
            "#!/bin/sh
",
        )
        .unwrap();
        make_executable(&target);
        let command = resolve_proxy_command(
            &[target.as_os_str().to_os_string()],
            dir.path(),
            Some(OsStr::new("")),
        )
        .unwrap();
        assert_eq!(command.program, target);
        assert!(command.args.is_empty());
    }

    #[test]
    fn rejects_ambiguous_typescript_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("server.ts");
        fs::write(&target, "export {};").unwrap();
        let error = resolve_proxy_command(
            &[target.as_os_str().to_os_string()],
            dir.path(),
            Some(OsStr::new("")),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProxyCommandError::AmbiguousTypeScriptRuntime { .. }
        ));
    }

    #[test]
    fn resolves_env_shebang() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let python = bin.join("python3");
        fs::write(&python, "").unwrap();
        make_executable(&python);
        let target = dir.path().join("server");
        fs::write(
            &target,
            "#!/usr/bin/env python3
",
        )
        .unwrap();
        let path = std::env::join_paths([&bin]).unwrap();
        let command = resolve_proxy_command(
            &[target.as_os_str().to_os_string()],
            dir.path(),
            Some(&path),
        )
        .unwrap();
        assert_eq!(command.program, python);
        assert_eq!(command.args, vec![target.into_os_string()]);
    }
}
