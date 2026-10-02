use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

mod cache;
mod types;
pub use types::*;
mod inputs;
mod validation;
pub use inputs::merge_snippet_input;
use inputs::parse_inputs_block;
pub use validation::{frontmatter, validate_snippet_body, validate_snippet_code};
use validation::{
    has_frontmatter, render_user_snippet_body, snippet_metadata_fields,
    validate_snippet_body_structure,
};
mod publication;
#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod reliability_tests;

use serde_json::{Map, Value};

use super::tool_declarations::{self, SnippetToolDeclarations};
use crate::error::ToolError;

#[cfg(test)]
mod tool_declaration_tests;

const SNIPPET_EXTENSIONS: &[&str] = &["md", "js"];

/// Hard storage ceiling for a snippet's *executable* code — the extracted
/// ```js block, or the whole file for bare `.js` snippets. Keep this aligned
/// with Code Mode's hard source ceiling instead of a smaller snippet-only
/// legacy cap. Hosts may still configure a lower `code_mode.max_source_bytes`,
/// which is enforced when the snippet executes.
const MAX_SNIPPET_CODE_BYTES: usize = crate::config::MAX_SOURCE_BYTES;

/// Upper bound on the whole snippet markdown file (frontmatter + prose + fenced
/// code). Snippets often front-load substantial agent context in prose that
/// never executes, so give that context a full extra Code Mode source budget
/// while still rejecting pathological files before parsing.
const MAX_SNIPPET_FILE_BYTES: usize = 2 * crate::config::MAX_SOURCE_BYTES;

/// Apply the same source ceiling to saved-snippet invocations on live and
/// fixture surfaces. Parameters count because they become part of the source.
pub fn wrap_snippet_with_input_bounded(
    code: &str,
    input: &Value,
    max_source_bytes: usize,
) -> Result<String, ToolError> {
    let input = serde_json::to_string(input).map_err(|e| ToolError::InvalidParam {
        message: format!("snippet params must be JSON-serializable: {e}"),
        param: "params".to_string(),
    })?;
    let wrapped = format!(
        "async () => {{\n  const __labSnippetInput = {input};\n  return await ({code})(__labSnippetInput);\n}}"
    );
    if wrapped.len() > max_source_bytes {
        return Err(ToolError::InvalidParam {
            message: format!(
                "saved snippet invocation exceeds Code Mode source limit {max_source_bytes} bytes after serializing params ({} bytes)",
                wrapped.len()
            ),
            param: "params".to_string(),
        });
    }
    Ok(wrapped)
}

/// Return the per-user snippet directory under the Labby home.
#[must_use]
pub fn user_snippet_dir(lab_home: &Path) -> PathBuf {
    lab_home.join("snippets")
}

/// Return the checked-in built-in snippet directory.
#[must_use]
pub fn builtin_snippet_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/snippets")
}

/// Validate a stable lowercase snippet identifier.
pub fn validate_snippet_name(name: &str) -> Result<(), ToolError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return invalid_name(name);
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return invalid_name(name);
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return invalid_name(name);
    }
    Ok(())
}

fn invalid_name(name: &str) -> Result<(), ToolError> {
    Err(ToolError::InvalidParam {
        message: format!(
            "invalid snippet name `{name}`; use lowercase letters, digits, hyphens, and underscores"
        ),
        param: "name".to_string(),
    })
}

/// Extract the first fenced JavaScript block from a Markdown snippet.
pub fn extract_javascript_block(source: &str) -> Result<String, ToolError> {
    let mut in_fence = false;
    let mut wanted = false;
    let mut body = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(info) = trimmed.strip_prefix("```") {
            if in_fence {
                if wanted {
                    return Ok(body.join("\n").trim().to_string());
                }
                in_fence = false;
                wanted = false;
                body.clear();
                continue;
            }

            let language = info.split_whitespace().next().unwrap_or_default();
            in_fence = true;
            wanted = matches!(language, "js" | "javascript");
            body.clear();
            continue;
        }

        if in_fence && wanted {
            body.push(line);
        }
    }

    Err(ToolError::InvalidParam {
        message: "snippet markdown must contain a fenced ```js or ```javascript block".to_string(),
        param: "body".to_string(),
    })
}

/// Resolve and validate the executable JavaScript source for a snippet.
pub fn code_for_snippet(snippet: &ResolvedSnippet) -> Result<String, ToolError> {
    let code = if snippet
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "js")
    {
        snippet.body.clone()
    } else if has_frontmatter(&snippet.body) || snippet.body.contains("```") {
        extract_javascript_block(&snippet.body)?
    } else {
        snippet.body.trim().to_string()
    };
    let code = normalize_snippet_code(&code).to_string();
    validate_snippet_code(&code)?;
    Ok(code)
}

fn normalize_snippet_code(code: &str) -> &str {
    let code = code.trim();
    code.strip_suffix(';').map_or(code, str::trim_end)
}

/// Validate and atomically create or replace a user Markdown snippet.
pub fn create_user_snippet(
    lab_home: &Path,
    name: &str,
    body: &str,
    description: Option<&str>,
    force: bool,
) -> Result<SnippetInfo, ToolError> {
    create_user_snippet_checked(lab_home, name, body, description, force, None)
}

/// Create or replace a snippet, optionally requiring the previous source digest.
/// The digest is checked under the same cross-process lock as publication.
pub fn create_user_snippet_checked(
    lab_home: &Path,
    name: &str,
    body: &str,
    description: Option<&str>,
    force: bool,
    expected_digest: Option<&str>,
) -> Result<SnippetInfo, ToolError> {
    validate_snippet_name(name)?;
    validate_snippet_body(name, body)?;
    let dir = user_snippet_dir(lab_home);
    fs::create_dir_all(&dir).map_err(|e| io_error("create snippets directory", &dir, e))?;
    let body = render_user_snippet_body(name, body, description)?;
    let path = publication::publish(&dir, name, &body, force, expected_digest)?;
    let (description, tags, inputs, tools) = snippet_metadata_fields(frontmatter(&body)?);
    Ok(SnippetInfo {
        content_digest: Some(cache::digest(&body)),
        tools,
        name: name.to_string(),
        description,
        tags,
        inputs,
        source: SnippetSource::User,
        path,
        shadowed: false,
    })
}

/// Create a user snippet from promoted execution source, enforcing built-in shadowing policy.
pub fn create_promoted_user_snippet(
    lab_home: &Path,
    builtin_dir: &Path,
    name: &str,
    code: &str,
    description: Option<&str>,
    force: bool,
    shadow_builtin: bool,
) -> Result<SnippetInfo, ToolError> {
    validate_snippet_name(name)?;
    validate_snippet_code(code)?;
    let shadows_builtin = find_snippet_file(builtin_dir, name).is_some();
    if shadows_builtin && !shadow_builtin {
        return Err(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: format!(
                "snippet `{name}` matches a built-in snippet; pass shadow_builtin:true to create a user override"
            ),
        });
    }
    let info = create_user_snippet(lab_home, name, code, description, force)?;
    Ok(SnippetInfo {
        shadowed: shadows_builtin,
        ..info
    })
}

/// List user and built-in snippets, marking built-ins shadowed by user overrides.
pub fn list_snippets(lab_home: &Path, builtin_dir: &Path) -> Result<Vec<SnippetInfo>, ToolError> {
    Ok(list_snippets_with_diagnostics(lab_home, builtin_dir)?.snippets)
}

/// List snippets with failures; invalid user overrides still shadow built-ins.
pub fn list_snippets_with_diagnostics(
    lab_home: &Path,
    builtin_dir: &Path,
) -> Result<SnippetList, ToolError> {
    let mut snippets = Vec::new();
    let mut diagnostics = Vec::new();
    let mut diagnostics_omitted = 0;
    let user_dir = user_snippet_dir(lab_home);
    let user_names = collect_snippets(
        &user_dir,
        SnippetSource::User,
        &mut snippets,
        &mut diagnostics,
        &mut diagnostics_omitted,
    )?;
    collect_snippets(
        builtin_dir,
        SnippetSource::Builtin,
        &mut snippets,
        &mut diagnostics,
        &mut diagnostics_omitted,
    )?;

    for snippet in &mut snippets {
        snippet.shadowed =
            snippet.source == SnippetSource::Builtin && user_names.contains(&snippet.name);
    }
    snippets.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| source_rank(a.source).cmp(&source_rank(b.source)))
    });
    Ok(SnippetList {
        snippets,
        diagnostics,
        diagnostics_omitted,
    })
}

/// Resolve a snippet by name, preferring a user override over the built-in copy.
pub fn resolve_snippet(
    lab_home: &Path,
    builtin_dir: &Path,
    name: &str,
) -> Result<ResolvedSnippet, ToolError> {
    validate_snippet_name(name)?;
    let user_dir = user_snippet_dir(lab_home);
    if let Some(path) = find_snippet_file(&user_dir, name) {
        return read_resolved(name, SnippetSource::User, path);
    }
    if let Some(path) = find_snippet_file(builtin_dir, name) {
        return read_resolved(name, SnippetSource::Builtin, path);
    }
    tracing::debug!(
        snippet = %name,
        user_dir = %user_dir.display(),
        builtin_dir = %builtin_dir.display(),
        "saved snippet was not found in configured authorities"
    );
    Err(ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("snippet `{name}` not found"),
    })
}

/// Remove a user snippet while refusing deletion of built-in snippets.
pub fn remove_user_snippet(
    lab_home: &Path,
    builtin_dir: &Path,
    name: &str,
) -> Result<SnippetRemoveResult, ToolError> {
    validate_snippet_name(name)?;
    let user_dir = user_snippet_dir(lab_home);
    let _lock = if user_dir.exists() {
        Some(publication::lock(&user_dir)?)
    } else {
        None
    };
    if let Some(path) = find_snippet_file(&user_dir, name) {
        fs::remove_file(&path).map_err(|e| io_error("remove snippet", &path, e))?;
        return Ok(SnippetRemoveResult {
            name: name.to_string(),
            removed: true,
        });
    }
    if find_snippet_file(builtin_dir, name).is_some() {
        return Err(ToolError::InvalidParam {
            message: format!("snippet `{name}` is built in; only user snippets can be removed"),
            param: "name".to_string(),
        });
    }
    Err(ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("user snippet `{name}` not found"),
    })
}

fn collect_snippets(
    dir: &Path,
    source: SnippetSource,
    out: &mut Vec<SnippetInfo>,
    diagnostics: &mut Vec<SnippetDiagnostic>,
    diagnostics_omitted: &mut usize,
) -> Result<std::collections::HashSet<String>, ToolError> {
    let mut names = std::collections::HashSet::new();
    if !dir.exists() {
        return Ok(names);
    }
    let entries = fs::read_dir(dir).map_err(|e| io_error("read snippets directory", dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_error("read snippets directory entry", dir, e))?;
        let path = entry.path();
        if !path.is_file() || !has_snippet_extension(&path) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if validate_snippet_name(stem).is_err() {
            continue;
        }
        names.insert(stem.to_string());
        // Discovery must describe the same effective extension as resolution.
        if find_snippet_file(dir, stem).as_deref() != Some(path.as_path()) {
            if diagnostics.len() < 128 {
                diagnostics.push(SnippetDiagnostic {
                    name: stem.to_string(),
                    path,
                    source,
                    message:
                        "inactive duplicate snippet extension; resolution uses the preferred extension"
                            .into(),
                });
            } else {
                *diagnostics_omitted += 1;
            }
            continue;
        }
        let metadata =
            read_snippet_body(&path).and_then(|body| cache::metadata(stem, &body, source));
        let (description, tags, inputs, tools, content_digest) = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                if diagnostics.len() < 128 {
                    diagnostics.push(SnippetDiagnostic {
                        name: stem.to_string(),
                        path,
                        source,
                        // List is public discovery: never expose source excerpts or raw I/O errors.
                        message: format!(
                            "snippet could not be loaded ({}); use admin validation for details",
                            error.kind()
                        ),
                    });
                } else {
                    *diagnostics_omitted += 1;
                }
                continue;
            }
        };
        out.push(SnippetInfo {
            content_digest: Some(content_digest),
            tools,
            name: stem.to_string(),
            description,
            tags,
            inputs,
            source,
            path,
            shadowed: false,
        });
    }
    Ok(names)
}

fn find_snippet_file(dir: &Path, name: &str) -> Option<PathBuf> {
    SNIPPET_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|path| path.is_file())
}

fn has_snippet_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| SNIPPET_EXTENSIONS.contains(&ext))
}

fn read_snippet_body(path: &Path) -> Result<String, ToolError> {
    let file = fs::File::open(path).map_err(|e| io_error("open snippet", path, e))?;
    let mut bytes = Vec::new();
    file.take((MAX_SNIPPET_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error("read snippet", path, e))?;
    if bytes.len() > MAX_SNIPPET_FILE_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!("snippet file exceeds {MAX_SNIPPET_FILE_BYTES} bytes"),
            param: "body".to_string(),
        });
    }
    String::from_utf8(bytes).map_err(|_| ToolError::InvalidParam {
        message: "snippet file must contain valid UTF-8".to_string(),
        param: "body".to_string(),
    })
}

fn read_resolved(
    name: &str,
    source: SnippetSource,
    path: PathBuf,
) -> Result<ResolvedSnippet, ToolError> {
    let body = read_snippet_body(&path)?;
    // Resolution is a host-side lookup, not a second execution validator. Keep
    // the file/frontmatter/size contract here, then let code_for_snippet() do
    // the single authoritative Javy parse immediately before execution (or an
    // explicit existing-snippet validation). This avoids compiling the same
    // saved program twice per invocation.
    validate_snippet_body_structure(name, &body)?;
    let (description, tags, inputs, tools) =
        snippet_metadata_fields(frontmatter(&body)?.filter(|m| m.name == name));
    Ok(ResolvedSnippet {
        content_digest: Some(cache::digest(&body)),
        tools,
        name: name.to_string(),
        description,
        tags,
        inputs,
        source,
        path,
        body,
    })
}

const fn source_rank(source: SnippetSource) -> u8 {
    match source {
        SnippetSource::User => 0,
        SnippetSource::Builtin => 1,
    }
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("{action} `{}` failed: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests;
