//! Verify installed companions before they participate in native setup or startup.
use anyhow::{Context as _, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Read as _,
    path::{Component, Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct VerifiedBundle {
    pub helper: PathBuf,
    pub helper_sha256: [u8; 32],
    pub adapter: PathBuf,
    pub browser: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    version: String,
    protocol: u32,
    target: String,
    components: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    sha256: String,
}

fn target() -> Result<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
        ("aarch64", "linux") => Ok("aarch64-unknown-linux-gnu"),
        ("x86_64", "linux") => Ok("x86_64-unknown-linux-gnu"),
        _ => bail!("no Tailcat release bundle for this platform"),
    }
}

pub(crate) fn discover(configured: Option<&Path>) -> Result<VerifiedBundle> {
    let root = match configured {
        Some(path) => path.to_owned(),
        None => std::env::current_exe()?
            .parent()
            .context("binary has no installation directory")?
            .join("tailcat"),
    };
    verify(&root, env!("CARGO_PKG_VERSION"), target()?)
        .context("verify installed Tailcat companions; install a matching Labby release bundle")
}

fn regular_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(
        !relative.is_empty() && !relative.contains(['\\', '\0']),
        "invalid companion path"
    );
    ensure!(
        relative
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."),
        "noncanonical companion path"
    );
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            bail!("companion path must be relative without traversal")
        };
        path.push(part);
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "symlink in companion path"
        );
    }
    ensure!(
        fs::metadata(&path)?.is_file(),
        "companion must be a regular file"
    );
    Ok(path)
}

fn verify(root: &Path, version: &str, target: &str) -> Result<VerifiedBundle> {
    let root = root
        .canonicalize()
        .context("installed companion directory missing")?;
    let manifest_path = regular_path(&root, "manifest.json")?;
    let mut bytes = Vec::new();
    fs::File::open(manifest_path)?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "companion manifest exceeds budget"
    );
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    ensure!(
        manifest.schema_version == 1 && manifest.protocol == 1,
        "unsupported companion protocol"
    );
    ensure!(
        manifest.version == version && manifest.target == target,
        "companion version or target mismatch"
    );
    ensure!(
        manifest.components.len() <= 8192,
        "companion count exceeds budget"
    );
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    let mut helper_digest = None;
    for entry in manifest.components {
        ensure!(seen.insert(entry.path.clone()), "duplicate companion path");
        let path = regular_path(&root, &entry.path)?;
        let length = fs::metadata(&path)?.len();
        total = total
            .checked_add(length)
            .context("companion size overflow")?;
        ensure!(
            length <= 128 * 1024 * 1024 && total <= 512 * 1024 * 1024,
            "companion bytes exceed budget"
        );
        let expected: [u8; 32] = hex::decode(&entry.sha256)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid companion SHA256"))?;
        let mut file = fs::File::open(path)?.take(length + 1);
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 65536];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        ensure!(
            <[u8; 32]>::from(hash.finalize()) == expected,
            "companion checksum mismatch: {}",
            entry.path
        );
        if entry.path == "native/tailcat-bridge" {
            helper_digest = Some(expected);
        }
    }
    let mut actual = HashSet::new();
    let mut pending = vec![(root.clone(), 0_usize)];
    let mut entries = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        ensure!(depth <= 64, "companion directory depth exceeds budget");
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            entries += 1;
            ensure!(entries <= 16384, "companion inventory exceeds budget");
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push((entry.path(), depth + 1));
            } else {
                ensure!(kind.is_file(), "unsupported companion entry");
                actual.insert(
                    entry
                        .path()
                        .strip_prefix(&root)?
                        .to_str()
                        .context("non UTF8 companion path")?
                        .to_owned(),
                );
            }
        }
    }
    actual.remove("manifest.json");
    ensure!(
        actual == seen,
        "installed companion inventory differs from manifest"
    );
    for required in [
        "native/tailcat-bridge",
        "browser/hook.mjs",
        "browser/transport.mjs",
        "browser/http.mjs",
        "browser/wasm.mjs",
        "browser/tailcat.wasm",
        "browser/tailcat.wasm.gz",
        "browser/wasm_exec.js",
        "browser/THIRD_PARTY_NOTICES.md",
        "adapter/server.mjs",
        "adapter/cleanup.mjs",
        "adapter/package.json",
        "adapter/package-lock.json",
    ] {
        ensure!(
            seen.contains(required),
            "missing required companion: {required}"
        );
    }
    Ok(VerifiedBundle {
        helper: root.join("native/tailcat-bridge"),
        helper_sha256: helper_digest.context("missing helper digest")?,
        adapter: root.join("adapter/server.mjs"),
        browser: root.join("browser"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_reject_traversal_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("safe"), b"asset").unwrap();
        assert!(regular_path(dir.path(), "safe").is_ok());
        for path in [
            "../safe",
            "/safe",
            "",
            "safe\\other",
            "./safe",
            "safe/../safe",
            "safe//other",
        ] {
            assert!(regular_path(dir.path(), path).is_err(), "{path}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("safe", dir.path().join("link")).unwrap();
            assert!(regular_path(dir.path(), "link").is_err());
        }
    }

    #[test]
    fn mismatched_or_incomplete_manifest_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        for (version, target, protocol) in [
            ("other", "target", 1),
            ("version", "other", 1),
            ("version", "target", 2),
            ("version", "target", 1),
        ] {
            fs::write(dir.path().join("manifest.json"), serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"version":version,"target":target,"protocol":protocol,"components":[]})).unwrap()).unwrap();
            assert!(verify(dir.path(), "version", "target").is_err());
        }
    }

    #[test]
    fn tampered_components_fail_before_required_inventory_check() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("asset"), b"tampered").unwrap();
        fs::write(dir.path().join("manifest.json"), serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"version":"version","target":"target","protocol":1,"components":[{"path":"asset","sha256":hex::encode(Sha256::digest(b"original"))}]})).unwrap()).unwrap();
        let error = verify(dir.path(), "version", "target").unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"));
    }
    #[test]
    fn complete_bundle_verifies_and_rejects_unlisted_dependency_override() {
        let dir = tempfile::tempdir().unwrap();
        let mut components = Vec::new();
        for relative in [
            "native/tailcat-bridge",
            "browser/hook.mjs",
            "browser/transport.mjs",
            "browser/http.mjs",
            "browser/wasm.mjs",
            "browser/tailcat.wasm",
            "browser/tailcat.wasm.gz",
            "browser/wasm_exec.js",
            "browser/THIRD_PARTY_NOTICES.md",
            "adapter/server.mjs",
            "adapter/cleanup.mjs",
            "adapter/package.json",
            "adapter/package-lock.json",
            "adapter/node_modules/microsandbox/index.js",
        ] {
            let path = dir.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, relative.as_bytes()).unwrap();
            components.push(serde_json::json!({
                "path": relative,
                "sha256": hex::encode(Sha256::digest(relative.as_bytes())),
            }));
        }
        fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1, "version": "version", "target": "target",
                "protocol": 1, "components": components,
            }))
            .unwrap(),
        )
        .unwrap();
        let bundle = verify(dir.path(), "version", "target").unwrap();
        assert_eq!(
            bundle.helper,
            dir.path()
                .canonicalize()
                .unwrap()
                .join("native/tailcat-bridge")
        );
        assert_eq!(
            bundle.adapter,
            dir.path()
                .canonicalize()
                .unwrap()
                .join("adapter/server.mjs")
        );
        let injected = dir
            .path()
            .join("adapter/node_modules/microsandbox/node_modules/zod/index.js");
        fs::create_dir_all(injected.parent().unwrap()).unwrap();
        fs::write(injected, b"unverified override").unwrap();
        let error = verify(dir.path(), "version", "target").unwrap_err();
        assert!(error.to_string().contains("inventory"), "{error}");
    }
}
