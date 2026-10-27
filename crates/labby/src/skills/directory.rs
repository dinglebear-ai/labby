//! Directory projection from already authorized, immutable skill manifests.

use labby_runtime::error::ToolError;
use labby_runtime::skills::wire::SkillsListResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const PAGE_SIZE: usize = 128;
const MAX_CHILDREN: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirectoryChild {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DirectoryParams {
    pub uri: String,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirectoryPage {
    result_type: &'static str,
    pub resources: Vec<DirectoryChild>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    ttl_ms: u64,
    cache_scope: &'static str,
}

fn invalid(param: &str) -> ToolError {
    ToolError::InvalidParam {
        message: "unknown skill directory or invalid directory cursor".into(),
        param: param.into(),
    }
}

pub(crate) fn directory_page(
    listing: &SkillsListResult,
    params: &DirectoryParams,
    visibility_key: &str,
) -> Result<DirectoryPage, ToolError> {
    labby_runtime::skills::parse_skill_uri(&params.uri).map_err(|_| invalid("uri"))?;
    let prefix = format!("{}/", params.uri);
    let mut children = BTreeMap::new();
    let mut digest = Sha256::new();
    digest.update(visibility_key.as_bytes());
    digest.update([0]);
    digest.update(params.uri.as_bytes());
    for skill in &listing.skills {
        digest.update(skill.uri.as_bytes());
        digest.update([0]);
        let resources = skill.resources.as_ref().ok_or_else(|| invalid("uri"))?;
        for resource in resources {
            digest.update(resource.uri.as_bytes());
            digest.update([0]);
            digest.update(resource.digest.as_bytes());
            digest.update(resource.size.to_be_bytes());
            if resource.uri == params.uri {
                return Err(invalid("uri"));
            }
            let Some(relative) = resource.uri.strip_prefix(&prefix) else {
                continue;
            };
            let (name, directory) = relative
                .split_once('/')
                .map_or((relative, false), |(name, _)| (name, true));
            let child = DirectoryChild {
                uri: format!("{prefix}{name}"),
                name: name.into(),
                mime_type: if directory {
                    Some("inode/directory".into())
                } else if name.ends_with(".md") {
                    Some("text/markdown".into())
                } else {
                    None
                },
            };
            if let Some(previous) = children.insert(child.uri.clone(), child.clone())
                && previous != child
            {
                return Err(invalid("uri"));
            }
            if children.len() > MAX_CHILDREN {
                return Err(ToolError::Sdk {
                    sdk_kind: "resource_exhausted".into(),
                    message: "skill directory exceeds the child count budget".into(),
                });
            }
        }
    }
    if children.is_empty() {
        return Err(invalid("uri"));
    }
    let digest = hex::encode(digest.finalize());
    let offset = match params.cursor.as_deref() {
        None => 0,
        Some(cursor) => {
            if cursor.len() > 128 {
                return Err(invalid("cursor"));
            }
            let prefix = format!("dir1:{digest}:");
            let offset = cursor
                .strip_prefix(&prefix)
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| invalid("cursor"))?;
            if offset == 0 || offset % PAGE_SIZE != 0 || offset >= children.len() {
                return Err(invalid("cursor"));
            }
            offset
        }
    };
    let end = (offset + PAGE_SIZE).min(children.len());
    let next_cursor = (end < children.len()).then(|| format!("dir1:{digest}:{end}"));
    Ok(DirectoryPage {
        result_type: "complete",
        resources: children
            .into_values()
            .skip(offset)
            .take(PAGE_SIZE)
            .collect(),
        next_cursor,
        ttl_ms: 0,
        cache_scope: "private",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::facade::{SkillRegistryContext, first_party_skill_listing};

    fn listing() -> SkillsListResult {
        first_party_skill_listing(&SkillRegistryContext::first_party_only())
    }
    fn params(uri: &str) -> DirectoryParams {
        DirectoryParams {
            uri: uri.into(),
            cursor: None,
        }
    }

    #[test]
    fn directory_children_are_direct_and_manifest_bound() {
        let listing = listing();
        let page = directory_page(&listing, &params("skill://labby/using-labby"), "owner").unwrap();
        assert!(page.resources.iter().any(|c| c.name == "SKILL.md"));
        let references = page
            .resources
            .iter()
            .find(|c| c.name == "references")
            .unwrap();
        assert_eq!(references.mime_type.as_deref(), Some("inode/directory"));
        assert!(!page.resources.iter().any(|c| c.name.contains('/')));
        let nested = directory_page(&listing, &params(&references.uri), "owner").unwrap();
        assert!(
            nested
                .resources
                .iter()
                .all(|c| c.mime_type.as_deref() != Some("inode/directory"))
        );
        let mut hidden = listing.clone();
        hidden.skills.clear();
        assert!(directory_page(&hidden, &params("skill://labby/using-labby"), "owner").is_err());
    }

    #[test]
    fn directory_rejects_files_traversal_unknown_and_cursor_reuse() {
        let listing = listing();
        for uri in [
            "skill://labby/using-labby/SKILL.md",
            "skill://labby/unknown",
            "skill://labby/using-labby/",
            "skill://labby/using-labby/../using-codemode",
            "skill://labby/using-labby/%2e%2e",
        ] {
            assert!(
                directory_page(&listing, &params(uri), "owner").is_err(),
                "{uri}"
            );
        }
        let mut forged = params("skill://labby/using-labby");
        forged.cursor = Some("forged".into());
        assert!(directory_page(&listing, &forged, "owner").is_err());
    }

    #[test]
    fn directory_cursor_is_bound_to_uri_and_manifest_bytes() {
        let mut listing = listing();
        let entry = &mut listing.skills[0];
        let root = entry.uri.rsplit_once('/').unwrap().0.to_owned();
        let file = entry.resources.as_ref().unwrap()[0].clone();
        for n in 0..140 {
            let mut resource = file.clone();
            resource.uri = format!("{root}/file-{n:03}.md");
            entry.resources.as_mut().unwrap().push(resource);
        }
        let first = directory_page(&listing, &params(&root), "owner").unwrap();
        assert_eq!(first.resources.len(), PAGE_SIZE);
        let mut second = params(&root);
        second.cursor = first.next_cursor;
        assert!(
            !directory_page(&listing, &second, "owner")
                .unwrap()
                .resources
                .is_empty()
        );
        assert!(directory_page(&listing, &second, "different-caller").is_err());
        listing.skills[0].resources.as_mut().unwrap()[0]
            .digest
            .push('0');
        assert!(directory_page(&listing, &second, "owner").is_err());
        second.uri = "skill://labby".into();
        assert!(directory_page(&listing, &second, "owner").is_err());
    }
}
