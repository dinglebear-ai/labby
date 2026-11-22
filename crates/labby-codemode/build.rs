//! Embed the shipped snippets and their fixture files alongside the kernel.
use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/snippets");
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut assets = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("md" | "js" | "json")
        ) {
            let name = path.file_name().unwrap().to_string_lossy();
            assets.push(format!(
                "({name:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../docs/snippets/\", {name:?}))),\n"
            ));
        }
    }
    assets.sort();
    assert!(!assets.is_empty(), "the shipped snippet directory is empty");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("snippet_assets.rs"),
        format!("&[{}]", assets.concat()),
    )
}
