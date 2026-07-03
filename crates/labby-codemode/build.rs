fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let mut plugin_build_root = PluginBuildRoot::create(&out)?;
    let staged_plugin = stage_plugin_crate(&manifest_dir, plugin_build_root.path())?;
    let plugin_manifest = staged_plugin.join("Cargo.toml");
    let plugin_target = plugin_build_root.path().join("target");
    let workspace_target = manifest_dir.join("../../target");
    std::fs::create_dir_all(&workspace_target)?;
    let plugin_cargo_home = workspace_target
        .canonicalize()?
        .join("labby-codemode-javy-plugin-cargo-home");
    std::fs::create_dir_all(&plugin_cargo_home)?;

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=javy-plugin/Cargo.toml");
    println!("cargo:rerun-if-changed=javy-plugin/Cargo.lock");
    println!("cargo:rerun-if-changed=javy-plugin/src/lib.rs");
    println!("cargo:rerun-if-changed=plugin.sha256");

    let mut command = plugin_build_command();
    command.env_clear();
    preserve_env(&mut command, "PATH");
    preserve_env(&mut command, "HOME");
    preserve_env(&mut command, "RUSTUP_HOME");
    let status = command
        .current_dir(&staged_plugin)
        .env("CARGO_BUILD_RUSTC_WRAPPER", "")
        .env("RUSTC_WRAPPER", "")
        .env("SCCACHE_DISABLE", "1")
        .env("RUSTFLAGS", "")
        .env("CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS", "")
        .env("CARGO_HOME", &plugin_cargo_home)
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            format!(
                "--remap-path-prefix={}=/labby-codemode-javy-plugin\u{1f}--remap-path-prefix={}=/labby-codemode-javy-plugin-cargo-home",
                plugin_build_root.path().display(),
                plugin_cargo_home.display()
            ),
        )
        .arg("--manifest-path")
        .arg(&plugin_manifest)
        .arg("--target-dir")
        .arg(&plugin_target)
        .status()?;
    if !status.success() {
        return Err("failed to build Code Mode Javy plugin".into());
    }

    let raw =
        std::fs::read(plugin_target.join("wasm32-wasip1/release/labby_codemode_javy_plugin.wasm"))?;
    let initialized = labby_codemode_build_support::preinitialize_javy_plugin(&raw)?;
    let actual = labby_codemode_build_support::sha256_hex(&initialized);
    let expected = std::fs::read_to_string(manifest_dir.join("plugin.sha256"))?;
    let expected = expected.trim();
    if expected.is_empty() {
        return Err("plugin.sha256 must not be empty".into());
    }
    if expected != actual {
        return Err(format!(
            "preinitialized plugin hash mismatch: expected {expected}, got {actual}"
        )
        .into());
    }

    std::fs::write(out.join("plugin.wasm"), initialized)?;
    println!("cargo:rustc-env=LABBY_CODEMODE_PLUGIN_SHA256={actual}");
    println!("cargo:warning=labby Code Mode plugin sha256 {actual}");
    plugin_build_root.cleanup();
    Ok(())
}

fn preserve_env(command: &mut std::process::Command, key: &str) {
    if let Ok(value) = std::env::var(key) {
        command.env(key, value);
    }
}

struct PluginBuildRoot {
    path: Option<std::path::PathBuf>,
}

impl PluginBuildRoot {
    fn create(out: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let path = out.join(format!("javy-plugin-build-{}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        std::fs::create_dir_all(&path)?;
        Ok(Self { path: Some(path) })
    }

    fn path(&self) -> &std::path::Path {
        self.path.as_deref().expect("plugin build root exists")
    }

    fn cleanup(&mut self) {
        if let Some(path) = self.path.take()
            && let Err(err) = std::fs::remove_dir_all(&path)
        {
            println!(
                "cargo:warning=failed to remove Code Mode plugin build root {}: {err}",
                path.display()
            );
        }
    }
}

impl Drop for PluginBuildRoot {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            drop(std::fs::remove_dir_all(path));
        }
    }
}

fn plugin_build_command() -> std::process::Command {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let toolchain =
        std::env::var("LABBY_CODEMODE_PLUGIN_TOOLCHAIN").unwrap_or_else(|_| "stable".to_string());
    let mut command = match toolchain.trim() {
        "" | "current" => std::process::Command::new(cargo),
        toolchain => {
            let mut command = std::process::Command::new("rustup");
            command.args(["run", toolchain, "cargo"]);
            command
        }
    };
    command.args([
        "build",
        "--target",
        "wasm32-wasip1",
        "--release",
        "--locked",
    ]);
    command
}

fn stage_plugin_crate(
    manifest_dir: &std::path::Path,
    out: &std::path::Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let source = manifest_dir.join("javy-plugin");
    let staged = out.join("javy-plugin-src");
    if staged.exists() {
        std::fs::remove_dir_all(&staged)?;
    }
    std::fs::create_dir_all(staged.join("src"))?;
    for path in ["Cargo.toml", "Cargo.lock"] {
        std::fs::copy(source.join(path), staged.join(path))?;
    }
    std::fs::copy(source.join("src/lib.rs"), staged.join("src/lib.rs"))?;
    Ok(staged)
}
