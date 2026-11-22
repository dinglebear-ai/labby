use std::fs;
use std::path::{Path, PathBuf};

use super::{
    BUILTIN_DIRECTORY, BUILTIN_FILES, SnippetDiagnostic, SnippetInfo, SnippetList, SnippetSource,
    SnippetSourceFormat, cache, find_snippet_file, has_snippet_extension, io_error,
    read_snippet_body, user_snippet_dir, validate_snippet_name,
};
use crate::error::ToolError;

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

fn collect_snippets(
    dir: &Path,
    source: SnippetSource,
    out: &mut Vec<SnippetInfo>,
    diagnostics: &mut Vec<SnippetDiagnostic>,
    diagnostics_omitted: &mut usize,
) -> Result<std::collections::HashSet<String>, ToolError> {
    let mut names = std::collections::HashSet::new();
    let paths: Box<dyn Iterator<Item = Result<PathBuf, ToolError>> + '_> = if dir
        == Path::new(BUILTIN_DIRECTORY)
    {
        Box::new(BUILTIN_FILES.iter().map(|(name, _)| Ok(dir.join(name))))
    } else {
        if !dir.exists() {
            return Ok(names);
        }
        let entries = fs::read_dir(dir).map_err(|e| io_error("read snippets directory", dir, e))?;
        Box::new(entries.filter_map(|entry| match entry {
            Ok(entry) if entry.path().is_file() => Some(Ok(entry.path())),
            Ok(_) => None,
            Err(error) => Some(Err(io_error("read snippets directory entry", dir, error))),
        }))
    };
    for path in paths {
        let path = path?;
        if !has_snippet_extension(&path) {
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
        let format = if path.extension().and_then(|extension| extension.to_str()) == Some("js") {
            SnippetSourceFormat::JavaScript
        } else {
            SnippetSourceFormat::Markdown
        };
        let metadata =
            read_snippet_body(&path).and_then(|body| cache::metadata(stem, &body, source, format));
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

const fn source_rank(source: SnippetSource) -> u8 {
    match source {
        SnippetSource::User => 0,
        SnippetSource::Builtin => 1,
    }
}
