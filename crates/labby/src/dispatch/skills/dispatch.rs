use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str, to_json};
use crate::skills::facade::{
    SkillCallerScope, SkillRegistryContext, get_visible_skill, list_visible_skills,
    read_visible_skill_file,
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

#[cfg(feature = "gateway")]
pub(crate) async fn dispatch_with_manager_scope(
    manager: std::sync::Arc<labby_gateway::gateway::manager::GatewayManager>,
    scope: SkillCallerScope,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let context = SkillRegistryContext::with_manager(manager, scope);
    dispatch_with_context(&context, action, params).await
}

pub(crate) async fn dispatch_with_context(
    context: &SkillRegistryContext,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
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
    let skill = get_visible_skill(context, &uri)
        .await
        .ok_or_else(|| not_found(&uri, "skill"))?;
    to_json(SkillGetResponse {
        skill: SkillSummary::from(skill),
    })
}

async fn read(context: &SkillRegistryContext, params: Value) -> Result<Value, ToolError> {
    let params = parse::<UriParams>(params)?;
    let uri = normalized_uri(params.uri)?;
    let file = read_visible_skill_file(context, &uri).await?;
    Ok(json!({
        "uri": file.uri,
        "skill_uri": file.skill_uri,
        "origin": file.origin,
        "mime_type": file.mime_type,
        "digest": file.digest,
        "text": file.text,
    }))
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
        let get = dispatch("skills.get", json!({ "uri": uri }))
            .await
            .expect("get");
        let read = dispatch("skills.read", json!({ "uri": uri }))
            .await
            .expect("read");
        assert_eq!(get["skill"]["uri"], uri);
        assert_eq!(read["skill_uri"], uri);
        assert_eq!(
            read["text"].as_str(),
            crate::skills::read_first_party_skill_file(uri)
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
}
