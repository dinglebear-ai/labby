//! Release-profile guard for the live end-to-end hooks.
//!
//! The `LABBY_E2E_*` environment hooks (`LABBY_E2E_TEAM_ID`,
//! `LABBY_E2E_BOOTSTRAP_STATIC_OWNER`, `LABBY_E2E_DETERMINISTIC_EXECUTORS`)
//! may only be consulted by product code that is compiled under the
//! `proxy-testkit` cargo feature. This test walks every non-test line of
//! `crates/labby/src` and fails when a hook is read without a `proxy-testkit`
//! gate in the immediately preceding lines, so a future edit cannot quietly
//! re-enable a hook in product builds. The in-crate `#[cfg(not(feature =
//! "proxy-testkit"))]` tests prove the compiled-out behaviour itself.

use std::path::{Path, PathBuf};

const HOOKS: [&str; 3] = [
    "LABBY_E2E_TEAM_ID",
    "LABBY_E2E_BOOTSTRAP_STATIC_OWNER",
    "LABBY_E2E_DETERMINISTIC_EXECUTORS",
];
const GATE: &str = "proxy-testkit";
/// How far above a hook read the gate must appear.
const WINDOW: usize = 8;

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// A hook *read* is a runtime environment lookup or a clap `env =` binding.
fn reads_hook(line: &str) -> bool {
    HOOKS.iter().any(|hook| line.contains(hook))
        && (line.contains("env::var") || line.contains("env = \"LABBY_E2E_"))
}

#[test]
fn every_e2e_hook_read_in_product_code_is_gated_on_proxy_testkit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    files.sort();
    let mut reads = 0;
    let mut violations = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read source");
        let lines: Vec<&str> = text.lines().collect();
        // Everything from `#[cfg(test)]` onwards in a file is test code and
        // may mention the hook freely (the guard tests themselves do).
        let test_start = lines
            .iter()
            .position(|line| line.trim() == "#[cfg(test)]")
            .unwrap_or(lines.len());
        for (index, line) in lines.iter().enumerate().take(test_start) {
            if !reads_hook(line) {
                continue;
            }
            reads += 1;
            let window_start = index.saturating_sub(WINDOW);
            let gated = lines[window_start..=index]
                .iter()
                .any(|previous| previous.contains(GATE));
            if !gated {
                violations.push(format!(
                    "{}:{}: {}",
                    file.strip_prefix(&root).unwrap().display(),
                    index + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        reads >= 3,
        "expected the three live hooks to be read somewhere in product code; found {reads}"
    );
    assert!(
        violations.is_empty(),
        "LABBY_E2E_* hooks read without a `{GATE}` gate:\n{}",
        violations.join("\n")
    );
}

#[test]
fn proxy_testkit_is_not_part_of_any_product_feature_slice() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("read Cargo.toml");
    let features = manifest
        .split("[features]")
        .nth(1)
        .expect("features table")
        .split("\n[")
        .next()
        .expect("features body");
    for line in features.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name == GATE {
            continue;
        }
        assert!(
            !value.contains(GATE),
            "product feature `{name}` must not enable `{GATE}`: {line}"
        );
    }
}
