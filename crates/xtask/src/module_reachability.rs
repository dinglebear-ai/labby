//! Guard against source files that exist on disk but are unreachable.
//!
//! Rust compiles a file only when some parent declares it with `mod`. A file
//! whose declaration is dropped stays on disk, compiles nowhere, and produces
//! no warning — `rustc` never sees it, so there is no `dead_code` lint to fire.
//!
//! This is not hypothetical. The `land:` merges that produced #502 and #503
//! dropped `mod catalog_publication;` and `mod skills_exposure;` from
//! `upstream/pool.rs` while leaving both files in place, which broke the build
//! in a way that read as missing types rather than missing modules. The same
//! merges silently orphaned `paginate.rs` and two `*_tests.rs` files; because
//! nothing referenced their symbols, that orphaning produced no error at all
//! and eleven tests simply stopped running for months.
//!
//! Compilation failures announce themselves. Orphaned tests do not, which is
//! why this check exists as its own gate rather than being left to the build.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One source file that no parent module declares.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Orphan {
    /// Repository-relative path of the unreachable file.
    file: String,
    /// Repository-relative path of the parent expected to declare it.
    expected_parent: String,
}

/// Collect every `mod NAME;` declared in `source`, plus every `#[path = "..."]`
/// target, which attaches a file whose name need not match the module name.
fn declared_names(source: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();

    for line in source.lines() {
        let line = line.trim();

        if let Some(rest) = line.strip_prefix("#[path") {
            // `#[path = "foo.rs"]` — take the quoted target verbatim.
            if let Some(open) = rest.find('"')
                && let Some(close) = rest[open + 1..].find('"')
            {
                paths.insert(rest[open + 1..open + 1 + close].to_string());
            }
            continue;
        }

        // `mod foo;`, `pub mod foo;`, `pub(crate) mod foo;` — but never
        // `mod foo {`, which is an inline module owning no file.
        let Some(rest) = line
            .strip_prefix("pub(crate) mod ")
            .or_else(|| line.strip_prefix("pub(super) mod "))
            .or_else(|| line.strip_prefix("pub mod "))
            .or_else(|| line.strip_prefix("mod "))
        else {
            continue;
        };
        if let Some(name) = rest.strip_suffix(';') {
            names.insert(name.trim().to_string());
        }
    }

    (names, paths)
}

/// The file that owns module declarations for files sitting in `dir`.
///
/// For a crate's `src/` that is `lib.rs` or `main.rs`; for `src/foo/` it is the
/// sibling `src/foo.rs`. The repo bans `mod.rs`, so it is not considered.
fn parent_module_file(dir: &Path, crate_src: &Path) -> Option<PathBuf> {
    if dir == crate_src {
        for entry in ["lib.rs", "main.rs"] {
            let candidate = dir.join(entry);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        return None;
    }
    let sibling = dir.with_extension("rs");
    sibling.is_file().then_some(sibling)
}

fn visit(dir: &Path, crate_src: &Path, repo_root: &Path, orphans: &mut Vec<Orphan>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut files = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }

    if let Some(parent) = parent_module_file(dir, crate_src) {
        let source = fs::read_to_string(&parent).unwrap_or_default();
        let (mut names, mut paths) = declared_names(&source);

        // A sibling may attach a file with `#[path]` rather than the parent
        // doing it — `gateway/dispatch.rs` owns `gateway/dispatch_tests.rs`
        // that way. Fold in every sibling's declarations so those are not
        // reported as orphans.
        for sibling in &files {
            if *sibling == parent {
                continue;
            }
            let sibling_source = fs::read_to_string(sibling).unwrap_or_default();
            let (sibling_names, sibling_paths) = declared_names(&sibling_source);
            names.extend(sibling_names);
            paths.extend(sibling_paths);
        }

        for file in &files {
            // The parent declares the children; it does not declare itself,
            // and a crate root is reached through Cargo, not through `mod`.
            if *file == parent {
                continue;
            }
            // A crate can have both a library and a binary root; neither is
            // reached through `mod`.
            if dir == crate_src
                && file
                    .file_name()
                    .is_some_and(|name| name == "lib.rs" || name == "main.rs")
            {
                continue;
            }
            let stem = file
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if names.contains(stem) || paths.iter().any(|target| target.ends_with(name)) {
                continue;
            }
            orphans.push(Orphan {
                file: relative(file, repo_root),
                expected_parent: relative(&parent, repo_root),
            });
        }
    }

    for subdir in subdirs {
        visit(&subdir, crate_src, repo_root, orphans);
    }
}

fn relative(path: &Path, repo_root: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every `.rs` file under `crates/*/src` that no parent module declares.
fn orphaned_modules(repo_root: &Path) -> Vec<Orphan> {
    let mut orphans = Vec::new();
    let Ok(crates) = fs::read_dir(repo_root.join("crates")) else {
        return orphans;
    };
    for entry in crates.flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            visit(&src, &src, repo_root, &mut orphans);
        }
    }
    orphans.sort();
    orphans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        // crates/xtask -> repository root
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("xtask lives two levels below the repository root")
            .to_path_buf()
    }

    #[test]
    fn every_source_file_is_reachable_from_a_module_declaration() {
        let orphans = orphaned_modules(&repo_root());
        assert!(
            orphans.is_empty(),
            "these files exist on disk but no parent declares them, so they \
             compile nowhere and any tests inside them never run:\n{}",
            orphans
                .iter()
                .map(|orphan| format!(
                    "  {} (declare it in {})",
                    orphan.file, orphan.expected_parent
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn a_dropped_module_declaration_is_detected() {
        let (names, paths) = declared_names("mod alpha;\npub mod beta;\n");
        assert!(names.contains("alpha") && names.contains("beta"));
        assert!(paths.is_empty());
        assert!(
            !names.contains("gamma"),
            "an undeclared module must not be treated as reachable"
        );
    }

    #[test]
    fn inline_modules_and_path_attributes_are_not_false_positives() {
        // `mod tests { .. }` owns no file and must not be read as a declaration.
        let (names, _) = declared_names("mod tests {\n    fn helper() {}\n}\n");
        assert!(names.is_empty(), "inline module must not claim a file");

        // `#[path]` attaches a file whose name differs from the module name.
        let (_, paths) = declared_names("#[path = \"dispatch_tests.rs\"]\nmod tests;\n");
        assert!(paths.contains("dispatch_tests.rs"));
    }
}
