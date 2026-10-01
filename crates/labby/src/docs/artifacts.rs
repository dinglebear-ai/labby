use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::projection::{build_docs_projection, workspace_root};
use super::render;
use super::types::DocsProjection;

#[derive(Debug, Clone)]
pub struct CheckOutcome {
    pub checked: usize,
    pub stale: Vec<String>,
}

struct Artifact {
    path: &'static str,
    source: &'static str,
    content: String,
}

pub fn generate() -> Result<CheckOutcome> {
    let root = workspace_root()?;
    let projection = build_docs_projection(&root)?;
    let artifacts = build_artifacts(&projection)?;
    validate_artifacts(&artifacts, &root)?;
    validate_builtin_snippets(&root)?;
    for artifact in &artifacts {
        let path = root.join(artifact.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, &artifact.content)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(CheckOutcome {
        checked: artifacts.len(),
        stale: Vec::new(),
    })
}

pub fn check() -> Result<CheckOutcome> {
    let root = workspace_root()?;
    let projection = build_docs_projection(&root)?;
    if !projection.feature_matrix.mismatches.is_empty() {
        let messages = projection
            .feature_matrix
            .mismatches
            .iter()
            .map(|mismatch| format!("{}: {}", mismatch.feature, mismatch.message))
            .collect::<Vec<_>>()
            .join("; ");
        bail!("feature matrix invariant mismatch: {messages}");
    }
    let artifacts = build_artifacts(&projection)?;
    validate_artifacts(&artifacts, &root)?;
    validate_builtin_snippets(&root)?;
    let mut stale = Vec::new();
    for artifact in &artifacts {
        if let Some(path) = stale_path(&root, artifact)? {
            stale.push(path);
        }
    }
    Ok(CheckOutcome {
        checked: artifacts.len(),
        stale,
    })
}

fn build_artifacts(projection: &DocsProjection) -> Result<Vec<Artifact>> {
    let mut artifacts = vec![
        artifact(
            "docs/generated/README.md",
            "crates/labby/src/docs/render.rs",
            String::new(),
        ),
        artifact(
            "docs/generated/service-catalog.md",
            "crates/labby/src/docs/projection.rs",
            render::service_catalog(&projection.service_catalog),
        ),
        artifact(
            "docs/generated/service-catalog.json",
            "crates/labby/src/docs/projection.rs",
            render::json(&projection.service_catalog)?,
        ),
        artifact(
            "docs/generated/env-reference.md",
            "crates/labby/src/docs/projection.rs",
            render::env_reference(&projection.env_reference),
        ),
        artifact(
            "docs/generated/env-reference.json",
            "crates/labby/src/docs/projection.rs",
            render::json(&projection.env_reference)?,
        ),
        artifact(
            "docs/generated/proxy-config-reference.md",
            "crates/labby/src/docs/projection.rs",
            render::proxy_config_reference(&projection.proxy_config_reference),
        ),
        artifact(
            "docs/generated/proxy-config-reference.json",
            "crates/labby/src/docs/projection.rs",
            render::json(&projection.proxy_config_reference)?,
        ),
        artifact(
            "docs/generated/action-catalog.md",
            "crates/labby/src/docs/action_catalog.rs",
            render::action_catalog(&projection.action_catalog),
        ),
        artifact(
            "docs/generated/action-catalog.json",
            "crates/labby/src/docs/action_catalog.rs",
            render::json(&projection.action_catalog)?,
        ),
        artifact(
            "docs/generated/cli-help.md",
            "crates/labby/src/cli/help.rs",
            render::cli_help(),
        ),
        artifact(
            "docs/generated/cli-help.json",
            "crates/labby/src/cli/help.rs",
            render::json(&crate::cli::help::inventory())?,
        ),
        artifact(
            "docs/generated/cli-migration.md",
            "crates/labby/src/cli/migration.rs",
            render::cli_migration(),
        ),
        artifact(
            "docs/generated/mcp-help.md",
            "crates/labby/src/catalog.rs",
            render::mcp_help(&projection.mcp_help),
        ),
        artifact(
            "docs/generated/mcp-help.json",
            "crates/labby/src/catalog.rs",
            render::json(&projection.mcp_help)?,
        ),
        artifact(
            "docs/generated/api-routes.md",
            "crates/labby/src/docs/routes.rs",
            render::api_routes(&projection.api_routes),
        ),
        artifact(
            "docs/generated/api-routes.json",
            "crates/labby/src/docs/routes.rs",
            render::json(&projection.api_routes)?,
        ),
        artifact(
            "docs/generated/openapi.json",
            "crates/labby/src/api/openapi.rs",
            ensure_newline(&projection.openapi_json),
        ),
        artifact(
            "docs/generated/feature-matrix.md",
            "crates/labby/src/docs/projection.rs",
            render::feature_matrix(&projection.feature_matrix),
        ),
        artifact(
            "docs/generated/feature-matrix.json",
            "crates/labby/src/docs/projection.rs",
            render::json(&projection.feature_matrix)?,
        ),
    ];
    // Include the index itself without maintaining a second artifact inventory.
    let readme = render::generated_readme(artifacts.iter().map(|item| (item.path, item.source)));
    artifacts[0].content = readme;
    Ok(artifacts)
}

fn artifact(path: &'static str, source: &'static str, content: String) -> Artifact {
    Artifact {
        path,
        source,
        content,
    }
}

fn ensure_newline(value: &str) -> String {
    if value.ends_with('\n') {
        value.to_string()
    } else {
        format!("{value}\n")
    }
}

fn stale_path(root: &Path, artifact: &Artifact) -> Result<Option<String>> {
    let path = root.join(artifact.path);
    match fs::read_to_string(&path) {
        Ok(current) if current == artifact.content => Ok(None),
        Ok(_) => Ok(Some(artifact.path.to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(Some(artifact.path.to_string()))
        }
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn validate_artifacts(artifacts: &[Artifact], root: &Path) -> Result<()> {
    let mut forbidden = vec!["-----BEGIN ".to_string(), "eyJ".to_string()];
    forbidden.push(root.display().to_string());
    if let Ok(canonical) = root.canonicalize() {
        forbidden.push(canonical.display().to_string());
    }
    for artifact in artifacts {
        for forbidden in &forbidden {
            if artifact.content.contains(forbidden) {
                bail!(
                    "generated artifact {} contains forbidden safety pattern {forbidden}",
                    artifact.path
                );
            }
        }
        if contains_generic_absolute_path(&artifact.content) {
            bail!(
                "generated artifact {} contains a generic absolute local path",
                artifact.path
            );
        }
    }
    Ok(())
}

fn contains_generic_absolute_path(content: &str) -> bool {
    content
        .split(|ch: char| {
            ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']')
        })
        .any(|token| {
            token.starts_with("/home/")
                || token.starts_with("/Users/")
                || token.starts_with("/tmp/")
                || token.starts_with("/build/")
                || token.starts_with("\\Users\\")
                || (token.len() > 2
                    && token.as_bytes()[1] == b':'
                    && token.as_bytes()[2] == b'\\'
                    && token.as_bytes()[0].is_ascii_alphabetic())
        })
}

#[cfg(feature = "gateway")]
fn validate_builtin_snippets(root: &Path) -> Result<()> {
    let snippet_dir = root.join("docs/snippets");
    let entries = fs::read_dir(&snippet_dir)
        .with_context(|| format!("failed to read {}", snippet_dir.display()))?;
    let mut valid = 0usize;
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", snippet_dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let body = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if !body.starts_with("---\n") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid snippet filename `{}`", path.display()))?;
        crate::dispatch::snippets::store::validate_snippet_body(name, &body)
            .with_context(|| format!("invalid built-in snippet {}", path.display()))?;
        valid += 1;
    }
    if valid == 0 {
        bail!(
            "no valid built-in snippets found in {}",
            snippet_dir.display()
        );
    }
    Ok(())
}

#[cfg(not(feature = "gateway"))]
fn validate_builtin_snippets(_root: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docs::secret_example_is_suspicious;

    #[test]
    fn generated_readme_covers_the_manifest_and_source_owners() {
        let root = workspace_root().expect("workspace root");
        let projection = build_docs_projection(&root).expect("documentation projection");
        let artifacts = build_artifacts(&projection).expect("generated artifacts");
        let readme = &artifacts[0];
        assert_eq!(readme.path, "docs/generated/README.md");
        let rows = readme
            .content
            .lines()
            .filter(|line| line.starts_with("| ["))
            .count();
        assert_eq!(
            rows,
            artifacts.len(),
            "one ownership row per declared artifact"
        );
        let mut paths = std::collections::BTreeSet::new();
        for item in &artifacts {
            assert!(paths.insert(item.path), "duplicate artifact: {}", item.path);
            assert!(
                root.join(item.source).is_file(),
                "missing source owner: {}",
                item.source
            );
            let name = item
                .path
                .strip_prefix("docs/generated/")
                .expect("generated path");
            assert!(readme.content.contains(&format!("| [{name}]({name}) |")));
            assert!(readme.content.contains(&format!("(../../{})", item.source)));
        }
    }

    #[test]
    fn generated_readme_distinguishes_binary_check_from_repository_gate() {
        let readme = render::generated_readme(std::iter::empty());
        assert!(readme.contains("The `just docs-check` recipe"));
        assert!(readme.contains("Markdown links and heading fragments"));
        assert!(readme.contains("The `labby docs check` subcommand"));
        assert!(readme.contains("does not test live service health"));
        assert!(!readme.contains("It does not run Markdown link checks"));
    }

    #[test]
    fn secret_lint_rejects_token_like_values() {
        let synthetic_secret_key = ["s", "k", "-placeholder"].concat();
        let synthetic_jwt_prefix = ["e", "y", "Jplaceholder"].concat();
        assert!(secret_example_is_suspicious(&synthetic_secret_key));
        assert!(secret_example_is_suspicious(&synthetic_jwt_prefix));
        assert!(!secret_example_is_suspicious("<openai_api_key>"));
    }

    #[test]
    fn safety_lint_rejects_local_paths() {
        let artifacts = vec![artifact(
            "docs/generated/test.md",
            "crates/labby/src/docs/artifacts.rs",
            "/home/jmagar/leak".to_string(),
        )];
        assert!(validate_artifacts(&artifacts, Path::new(".")).is_err());
        let artifacts = vec![artifact(
            "docs/generated/test.md",
            "crates/labby/src/docs/artifacts.rs",
            "/tmp/leak".to_string(),
        )];
        assert!(validate_artifacts(&artifacts, Path::new(".")).is_err());
        let artifacts = vec![artifact(
            "docs/generated/test.md",
            "crates/labby/src/docs/artifacts.rs",
            "C:\\leak".to_string(),
        )];
        assert!(validate_artifacts(&artifacts, Path::new(".")).is_err());
    }
}
