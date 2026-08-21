use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dispatch::error::ToolError;

pub(crate) const DEFAULT_LIST_LIMIT: usize = 100;
pub(crate) const MAX_LIST_LIMIT: usize = 500;
pub(crate) const DEFAULT_SEARCH_LIMIT: usize = 20;
pub(crate) const MAX_SEARCH_LIMIT: usize = 100;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ListParams {
    pub(crate) origin: Option<String>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchParams {
    pub(crate) query: String,
    pub(crate) origin: Option<String>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UriParams {
    pub(crate) uri: String,
}

pub(crate) fn parse<T: DeserializeOwned>(params: Value) -> Result<T, ToolError> {
    serde_json::from_value(params).map_err(|error| ToolError::InvalidParam {
        message: format!("invalid parameters: {error}"),
        param: "params".to_string(),
    })
}

pub(crate) fn list_limit(limit: Option<usize>) -> Result<usize, ToolError> {
    bounded_limit(limit.unwrap_or(DEFAULT_LIST_LIMIT), MAX_LIST_LIMIT)
}

pub(crate) fn search_limit(limit: Option<usize>) -> Result<usize, ToolError> {
    bounded_limit(limit.unwrap_or(DEFAULT_SEARCH_LIMIT), MAX_SEARCH_LIMIT)
}

pub(crate) fn normalized_query(query: &str) -> Result<String, ToolError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "parameter query must not be empty".to_string(),
            param: "query".to_string(),
        });
    }
    Ok(query.to_string())
}

pub(crate) fn normalized_origin(origin: Option<String>) -> Result<Option<String>, ToolError> {
    match origin {
        None => Ok(None),
        Some(origin) if origin.trim().is_empty() => Err(ToolError::InvalidParam {
            message: "parameter origin must not be empty".to_string(),
            param: "origin".to_string(),
        }),
        Some(origin) => Ok(Some(origin)),
    }
}

pub(crate) fn normalized_uri(uri: String) -> Result<String, ToolError> {
    let uri = uri.trim();
    if uri.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "parameter uri must not be empty".to_string(),
            param: "uri".to_string(),
        });
    }
    labby_runtime::skills::parse_skill_resource_uri(uri).map_err(|error| {
        ToolError::InvalidParam {
            message: error.to_string(),
            param: "uri".to_string(),
        }
    })?;
    Ok(uri.to_string())
}

fn bounded_limit(limit: usize, maximum: usize) -> Result<usize, ToolError> {
    if limit == 0 || limit > maximum {
        return Err(ToolError::InvalidParam {
            message: format!("parameter limit must be between 1 and {maximum}"),
            param: "limit".to_string(),
        });
    }
    Ok(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_bounded() {
        assert_eq!(list_limit(None).unwrap(), DEFAULT_LIST_LIMIT);
        assert_eq!(search_limit(None).unwrap(), DEFAULT_SEARCH_LIMIT);
        assert!(list_limit(Some(0)).is_err());
        assert!(list_limit(Some(MAX_LIST_LIMIT + 1)).is_err());
        assert!(search_limit(Some(MAX_SEARCH_LIMIT + 1)).is_err());
    }

    #[test]
    fn query_and_uri_require_content() {
        assert!(normalized_query("  ").is_err());
        assert_eq!(normalized_query("  git  ").unwrap(), "git");
        assert!(normalized_uri(" ".to_string()).is_err());
        assert!(normalized_uri("not-a-uri".to_string()).is_err());
    }
}
