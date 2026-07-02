fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=javy-plugin/Cargo.toml");
    println!("cargo:rerun-if-changed=javy-plugin/src/lib.rs");
    println!("cargo:rerun-if-changed=plugin.sha256");

    let status = std::process::Command::new("rustup")
        .args([
            "run",
            "nightly",
            "cargo",
            "build",
            "--manifest-path",
            "javy-plugin/Cargo.toml",
            "--target",
            "wasm32-wasip1",
            "--release",
            "--locked",
        ])
        .env("CARGO_BUILD_RUSTC_WRAPPER", "")
        .env("RUSTC_WRAPPER", "")
        .env("SCCACHE_DISABLE", "1")
        .env("RUSTFLAGS", "")
        .env("CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS", "")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()?;
    if !status.success() {
        return Err("failed to build Code Mode Javy plugin".into());
    }

    let raw =
        std::fs::read("javy-plugin/target/wasm32-wasip1/release/labby_codemode_javy_plugin.wasm")?;
    let initialized = labby_codemode_build_support::preinitialize_javy_plugin(&raw)?;
    let actual = labby_codemode_build_support::sha256_hex(&initialized);
    let expected = std::fs::read_to_string("plugin.sha256").unwrap_or_default();
    let expected = expected.trim();
    if !expected.is_empty() && expected != actual {
        return Err(format!(
            "preinitialized plugin hash mismatch: expected {expected}, got {actual}"
        )
        .into());
    }

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(out.join("plugin.wasm"), initialized)?;
    println!("cargo:rustc-env=LABBY_CODEMODE_PLUGIN_SHA256={actual}");
    println!("cargo:warning=labby Code Mode plugin sha256 {actual}");
    Ok(())
}
