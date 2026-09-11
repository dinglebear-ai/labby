//! Executable form of the crate-boundary rules in `verification/CLAUDE.md`.
//!
//! The layering is the one thing in this workspace that cannot be recovered
//! once it is lost: a cycle or an upward dependency is cheap to add by accident
//! and expensive to unpick after code depends on it. `verify-scenario` reaching
//! for `verify-runner` is the specific mistake this guards — the replay-driven
//! normalization passes live in the runner precisely because the reverse
//! direction would be a cycle, and a future contributor moving them "back where
//! they belong" would discover that only at link time, if at all.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

/// Workspace-internal dependencies each crate is permitted to have.
const ALLOWED: &[(&str, &[&str])] = &[
    ("verify-core", &[]),
    ("verify-scenario", &["verify-core"]),
    ("verify-report", &["verify-core", "verify-scenario"]),
    (
        "verify-runner",
        &["verify-core", "verify-scenario", "verify-report"],
    ),
];

fn workspace_root() -> PathBuf {
    // .../verification/crates/verify-runner -> .../verification
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("verification workspace root above crates/verify-runner")
        .to_path_buf()
}

fn workspace_dependencies() -> BTreeMap<String, BTreeSet<String>> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(workspace_root())
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata");
    let packages = metadata["packages"]
        .as_array()
        .expect("metadata packages array");

    let members: BTreeSet<String> = packages
        .iter()
        .map(|package| package["name"].as_str().expect("package name").to_owned())
        .collect();

    packages
        .iter()
        .map(|package| {
            let name = package["name"].as_str().expect("package name").to_owned();
            let deps = package["dependencies"]
                .as_array()
                .expect("package dependencies")
                .iter()
                .filter_map(|dep| dep["name"].as_str())
                // Only workspace-internal edges are the concern here; third
                // party crates are governed by deny.toml instead.
                .filter(|dep| members.contains(*dep))
                .map(ToOwned::to_owned)
                .collect();
            (name, deps)
        })
        .collect()
}

#[test]
fn every_workspace_member_is_covered_by_the_layering_table() {
    let actual = workspace_dependencies();
    let declared: BTreeSet<&str> = ALLOWED.iter().map(|(name, _)| *name).collect();
    let present: BTreeSet<&str> = actual.keys().map(String::as_str).collect();
    assert_eq!(
        declared, present,
        "a workspace member was added or removed without updating the layering \
         table in this test; an uncovered crate is an unchecked crate"
    );
}

#[test]
fn no_crate_depends_upward_or_sideways_outside_its_layer() {
    let actual = workspace_dependencies();
    for (crate_name, allowed) in ALLOWED {
        let Some(deps) = actual.get(*crate_name) else {
            continue; // covered by the completeness test above
        };
        let allowed: BTreeSet<&str> = allowed.iter().copied().collect();
        let found: BTreeSet<&str> = deps.iter().map(String::as_str).collect();
        let violations: Vec<&&str> = found.difference(&allowed).collect();
        assert!(
            violations.is_empty(),
            "{crate_name} depends on {violations:?}, which the crate-boundary \
             rules in verification/CLAUDE.md do not permit"
        );
    }
}

#[test]
fn verify_core_stays_a_dependency_leaf() {
    let actual = workspace_dependencies();
    let deps = actual
        .get("verify-core")
        .expect("verify-core is a workspace member");
    assert!(
        deps.is_empty(),
        "verify-core must remain the dependency leaf, but depends on {deps:?}. \
         It is the crate every adopting project's vocabulary flows through, and \
         it stays transport-free, filesystem-free, and environment-free."
    );
}
