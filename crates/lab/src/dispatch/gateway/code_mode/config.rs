//! Shared Code Mode dispatch constants.
//!
//! Single source of truth for values that would otherwise drift between the
//! CLI and MCP surfaces (and between Rust host-side enforcement and the JS
//! preamble). Importing these here keeps surface parity a compile-time fact
//! instead of a convention.

/// Tracing `service` field for every Code Mode dispatch event.
///
/// Per `docs/dev/OBSERVABILITY.md` the `service` field is the aggregation key
/// for log/metric queries, so it must be identical across every Code Mode
/// surface. Use this constant for all `service = ...` tracing labels in the
/// Code Mode subsystem rather than a hand-typed string literal.
pub(crate) const SERVICE: &str = "code_mode";

/// Maximum accepted Code Mode source size in bytes (CLI and MCP).
///
/// Both surfaces enforce this identical `usize` limit so that the same source
/// is accepted or rejected regardless of entry point. The CLI historically
/// used `20 * 1024` (20480) and the MCP path `20_000`; they are unified here.
pub(crate) const MAX_SOURCE_BYTES: usize = 20_000;

/// Maximum `codemode.run(...)` snippet resolutions allowed in a single run.
///
/// Enforced on the host side in `runner_drive.rs` AND interpolated into the JS
/// preamble (`runner.rs::wrap_code_mode`, as `__labSnippetMaxResolves`) so the
/// in-sandbox guard and the host guard cannot drift. Edit this one place.
pub(in crate::dispatch::gateway::code_mode) const MAX_SNIPPET_RESOLVES_PER_RUN: usize = 32;

/// Maximum total bytes of resolved snippet source allowed in a single run.
///
/// Like [`MAX_SNIPPET_RESOLVES_PER_RUN`], enforced host-side in
/// `runner_drive.rs` and interpolated into the JS preamble (as
/// `__labSnippetMaxBytes`) so both sides share one definition.
pub(in crate::dispatch::gateway::code_mode) const MAX_SNIPPET_RESOLVED_BYTES_PER_RUN: usize =
    256 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_label_is_canonical() {
        assert_eq!(
            SERVICE, "code_mode",
            "the canonical Code Mode aggregation key must be `code_mode`"
        );
    }

    #[test]
    fn max_source_bytes_is_stable() {
        assert_eq!(MAX_SOURCE_BYTES, 20_000);
    }

    // Grep gate (lab-eozvy): no Code Mode dispatch event may emit the legacy
    // service label that is invisible to `service=code_mode` dashboards/alerts.
    // Walk the crate sources and fail on any reappearance. The needle is built
    // from fragments so this gate does not match its own source.
    #[test]
    fn no_legacy_codemode_service_label_in_sources() {
        let needle = format!("service = {:?}", "code".to_string() + "mode");
        let crate_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        visit_rust_files(&crate_src, &mut |path, contents| {
            for (idx, line) in contents.lines().enumerate() {
                if line.contains(&needle) {
                    offenders.push(format!("{}:{}", path.display(), idx + 1));
                }
            }
        });
        assert!(
            offenders.is_empty(),
            "legacy `{needle}` label found (use the SERVICE const): {offenders:?}"
        );
    }

    fn visit_rust_files(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path, &str)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit_rust_files(&path, f);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && let Ok(contents) = std::fs::read_to_string(&path)
            {
                f(&path, &contents);
            }
        }
    }
}
