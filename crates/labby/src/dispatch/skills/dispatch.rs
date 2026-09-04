use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};
use crate::skills::facade::{
    SkillRegistryContext, list_visible_skills, read_visible_skill_file, resolve_visible_skill,
};
use crate::skills::search::search_skill_entries;

use super::catalog::ACTIONS;
use super::client::first_party_context;
use super::params::{
    ListParams, SearchParams, UriParams, list_limit, normalized_origin, normalized_query,
    normalized_uri, parse, search_limit,
};
use super::types::{
    SkillGetResponse, SkillListResponse, SkillSearchResponse, SkillSearchResponseHit, SkillSummary,
    sort_entries,
};

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let context = first_party_context();
    dispatch_with_context(&context, action, params).await
}

pub(crate) async fn dispatch_with_context(
    context: &SkillRegistryContext,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    tracing::debug!(
        surface = "dispatch",
        service = "skills",
        action,
        skill_generation = context.generation_id(),
        skill_generation_digest = context.generation_digest(),
        "dispatching against captured Skill generation"
    );
    match action {
        "help" => Ok(help_payload("skills", ACTIONS)),
        "schema" => {
            let action = require_str(&params, "action")?;
            action_schema(ACTIONS, action)
        }
        "skills.list" => list(context, params).await,
        "skills.search" => search(context, params).await,
        "skills.get" => get(context, params).await,
        "skills.read" => read(context, params).await,
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action '{unknown}' for service 'skills'"),
            valid: ACTIONS
                .iter()
                .map(|action| action.name.to_string())
                .collect(),
            hint: None,
        }),
    }
}

async fn list(context: &SkillRegistryContext, params: Value) -> Result<Value, ToolError> {
    let params = if params.is_null() {
        ListParams::default()
    } else {
        parse::<ListParams>(params)?
    };
    let origin = normalized_origin(params.origin)?;
    let limit = list_limit(params.limit)?;
    let listing = list_visible_skills(context).await;
    let mut entries = listing.skills;
    if let Some(origin) = origin.as_deref() {
        entries.retain(|entry| entry_origin(entry).as_deref() == Some(origin));
    }
    sort_entries(&mut entries);
    let limit_truncated = entries.len() > limit;
    entries.truncate(limit);
    let skills = entries
        .into_iter()
        .map(SkillSummary::from)
        .collect::<Vec<_>>();
    let truncated = limit_truncated
        || listing
            .meta
            .as_ref()
            .and_then(|meta| meta.get("truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    to_json(SkillListResponse {
        total_returned: skills.len(),
        skills,
        incomplete: listing.meta,
        truncated,
    })
}

async fn search(context: &SkillRegistryContext, params: Value) -> Result<Value, ToolError> {
    let params = parse::<SearchParams>(params)?;
    let query = normalized_query(&params.query)?;
    let origin = normalized_origin(params.origin)?;
    let limit = search_limit(params.limit)?;
    let listing = list_visible_skills(context).await;
    let mut entries = listing.skills;
    if let Some(origin) = origin.as_deref() {
        entries.retain(|entry| entry_origin(entry).as_deref() == Some(origin));
    }
    let matches = search_skill_entries(&entries, &query, limit)
        .into_iter()
        .map(|hit| SkillSearchResponseHit {
            score: hit.score,
            match_fields: hit.match_fields,
            skill: SkillSummary::from(hit.skill),
        })
        .collect::<Vec<_>>();
    to_json(SkillSearchResponse {
        query,
        total_returned: matches.len(),
        matches,
        incomplete: listing.meta,
    })
}

async fn get(context: &SkillRegistryContext, params: Value) -> Result<Value, ToolError> {
    let params = parse::<UriParams>(params)?;
    let uri = normalized_uri(params.uri)?;
    let skill = resolve_visible_skill(context, &uri)
        .await?
        .ok_or_else(|| not_found(&uri, "skill"))?;
    to_json(SkillGetResponse {
        skill: SkillSummary::from(skill),
    })
}

async fn read(context: &SkillRegistryContext, params: Value) -> Result<Value, ToolError> {
    let params = parse::<UriParams>(params)?;
    let uri = normalized_uri(params.uri)?;
    let file = read_visible_skill_file(context, &uri).await?;
    Ok(visible_skill_file_to_json(file))
}

fn visible_skill_file_to_json(file: crate::skills::facade::VisibleSkillFile) -> Value {
    let text = file.content.text();
    let blob = file.content.encoded_blob();
    json!({
        "uri": file.uri,
        "skill_uri": file.skill_uri,
        "origin": file.origin,
        "mime_type": file.mime_type,
        "digest": file.digest,
        "text": text,
        "blob": blob,
    })
}

fn entry_origin(entry: &labby_runtime::skills::wire::SkillEntry) -> Option<String> {
    labby_runtime::skills::parse_skill_uri(&entry.uri)
        .ok()
        .map(|uri| uri.origin().to_string())
}

fn not_found(uri: &str, kind: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("no caller-visible {kind} owns '{uri}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_read_represents_binary_once_as_blob() {
        use crate::skills::facade::{VisibleSkillContent, VisibleSkillFile};

        let wire = visible_skill_file_to_json(VisibleSkillFile {
            uri: "skill://up/demo/asset.png".into(),
            skill_uri: "skill://up/demo/SKILL.md".into(),
            origin: "up".into(),
            digest: "sha256:test".into(),
            mime_type: Some("image/png".into()),
            content: VisibleSkillContent::Blob(vec![0, 1, 2, 3]),
        });
        assert!(wire["text"].is_null());
        assert_eq!(wire["blob"], "AAECAw==");
        assert_eq!(wire["mime_type"], "image/png");
    }

    fn write_versioned_skill(root: &std::path::Path, version: &str) {
        let dir = root.join("changing");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: changing\ndescription: {version}\n---\n\n{version}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), version).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn resident_set_bytes() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        Some(
            line.split_whitespace()
                .nth(1)?
                .parse::<u64>()
                .ok()?
                .saturating_mul(1024),
        )
    }

    #[tokio::test]
    async fn list_is_metadata_only_and_deterministic() {
        let value = dispatch("skills.list", json!({ "limit": 10 }))
            .await
            .expect("list");
        let skills = value["skills"].as_array().expect("skills");
        assert!(!skills.is_empty());
        assert!(skills.iter().all(|skill| skill.get("text").is_none()));
        let uris = skills
            .iter()
            .map(|skill| skill["uri"].as_str().unwrap())
            .collect::<Vec<_>>();
        let mut sorted = uris.clone();
        sorted.sort_unstable();
        assert_eq!(uris, sorted);
    }

    #[tokio::test]
    async fn search_finds_bundled_skill_without_loading_body() {
        let value = dispatch(
            "skills.search",
            json!({ "query": "using-labby", "limit": 5 }),
        )
        .await
        .expect("search");
        assert_eq!(value["matches"][0]["skill"]["name"], "using-labby");
        assert!(value["matches"][0].get("text").is_none());
    }

    #[tokio::test]
    async fn get_and_read_share_one_first_party_identity() {
        let uri = "skill://labby/using-labby/SKILL.md";
        let context = first_party_context();
        let get = dispatch_with_context(&context, "skills.get", json!({ "uri": uri }))
            .await
            .expect("get");
        let read = dispatch_with_context(&context, "skills.read", json!({ "uri": uri }))
            .await
            .expect("read");
        assert_eq!(get["skill"]["uri"], uri);
        assert_eq!(read["skill_uri"], uri);
        let digest = get["skill"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|resource| resource["uri"] == uri)
            .unwrap()["digest"]
            .as_str()
            .unwrap();
        assert_eq!(read["digest"], digest);
        assert!(
            labby_runtime::skills::parse_digest(digest)
                .unwrap()
                .matches(read["text"].as_str().unwrap().as_bytes())
        );
    }

    #[tokio::test]
    async fn context_free_dispatch_cannot_guess_an_upstream_origin() {
        let result = dispatch(
            "skills.get",
            json!({ "uri": "skill://private/skill/x/SKILL.md" }),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), "not_found");
    }

    #[tokio::test]
    async fn compatibility_routes_remain_pinned_across_a_refresh() {
        use crate::skills::facade::SkillRegistryContext;
        use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};

        let temp = tempfile::tempdir().unwrap();
        write_versioned_skill(temp.path(), "version-one");
        let manager = FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        );
        let old = SkillRegistryContext::from_generation(manager.generation());
        write_versioned_skill(temp.path(), "version-two");
        manager.refresh(None).unwrap();
        let new = SkillRegistryContext::from_generation(manager.generation());

        let manifest = "skill://labby/changing/SKILL.md";
        let notes = "skill://labby/changing/notes.md";
        let old_list = dispatch_with_context(&old, "skills.list", json!({ "limit": 10 }))
            .await
            .unwrap();
        let old_search = dispatch_with_context(
            &old,
            "skills.search",
            json!({ "query": "version-one", "limit": 10 }),
        )
        .await
        .unwrap();
        let old_get = dispatch_with_context(&old, "skills.get", json!({ "uri": notes }))
            .await
            .unwrap();
        let old_read = dispatch_with_context(&old, "skills.read", json!({ "uri": notes }))
            .await
            .unwrap();
        assert!(
            old_list["skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|skill| { skill["uri"] == manifest && skill["description"] == "version-one" })
        );
        assert_eq!(old_search["matches"][0]["skill"]["uri"], manifest);
        assert_eq!(old_get["skill"]["uri"], manifest);
        assert_eq!(old_read["text"], "version-one");

        let new_read = dispatch_with_context(&new, "skills.read", json!({ "uri": notes }))
            .await
            .unwrap();
        assert_eq!(new_read["text"], "version-two");
        assert_ne!(old_read["digest"], new_read["digest"]);
        assert_ne!(old.generation_id(), new.generation_id());
    }

    #[tokio::test]
    #[ignore = "allocation/scale regression harness; run explicitly on a quiet host"]
    async fn list_256_by_64_and_max_read_stay_bounded() {
        use crate::skills::facade::SkillRegistryContext;
        use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};
        use labby_runtime::skills::limits;

        let temp = tempfile::tempdir().unwrap();
        for skill in 0..256 {
            let name = format!("bench-{skill:03}");
            let dir = temp.path().join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: benchmark\n---\n"),
            )
            .unwrap();
            for resource in 1..limits::MAX_RESOURCES_PER_SKILL {
                let bytes = if skill == 0 && resource == 1 {
                    vec![b'x'; limits::MAX_SKILL_RESOURCE_BYTES]
                } else {
                    b"x".to_vec()
                };
                std::fs::write(dir.join(format!("resource-{resource:02}.txt")), bytes).unwrap();
            }
        }
        let caps = GenerationLimits {
            active_skills: 300,
            aggregate_bytes: 128 * 1024 * 1024,
            total_resources: 300 * limits::MAX_RESOURCES_PER_SKILL,
            live_candidate_bytes: 128 * 1024 * 1024,
            ..GenerationLimits::default()
        };
        let manager = FirstPartyGenerationManager::new(temp.path().to_path_buf(), caps);
        let context = SkillRegistryContext::from_generation(manager.generation());
        let started = std::time::Instant::now();
        let listing = dispatch_with_context(&context, "skills.list", json!({ "limit": 256 }))
            .await
            .unwrap();
        assert_eq!(listing["skills"].as_array().unwrap().len(), 256);

        #[cfg(target_os = "linux")]
        let rss_before = resident_set_bytes();
        let uri = "skill://labby/bench-000/resource-01.txt";
        let get = dispatch_with_context(&context, "skills.get", json!({ "uri": uri }))
            .await
            .unwrap();
        let read = dispatch_with_context(&context, "skills.read", json!({ "uri": uri }))
            .await
            .unwrap();
        assert_eq!(get["skill"]["uri"], "skill://labby/bench-000/SKILL.md");
        assert_eq!(
            read["text"].as_str().unwrap().len(),
            limits::MAX_SKILL_RESOURCE_BYTES
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        #[cfg(target_os = "linux")]
        if let (Some(before), Some(after)) = (rss_before, resident_set_bytes()) {
            // get clones one 64-resource descriptor and read owns one final
            // response body; neither operation may copy the complete corpus.
            let ceiling = (limits::MAX_SKILL_RESOURCE_BYTES as u64 * 4) + (8 * 1024 * 1024);
            assert!(after.saturating_sub(before) <= ceiling);
        }
    }
}
