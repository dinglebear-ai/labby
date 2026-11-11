//! Native RMCP request/result adaptation for Labby's independent Skills DTOs.

use rmcp::RoleClient;
use rmcp::model::{ClientRequest, ServerResult};
use rmcp::service::{Peer, ServiceError};
use serde::de::DeserializeOwned;

pub(super) async fn request<T: DeserializeOwned>(
    peer: &Peer<RoleClient>,
    request: ClientRequest,
) -> Result<T, ServiceError> {
    decode(peer.send_request(request).await?)
}

fn decode<T: DeserializeOwned>(result: ServerResult) -> Result<T, ServiceError> {
    // Native results carry metadata. The SDK's CustomResult fallback also keeps
    // future cache hints and individually malformed entries for Labby's existing
    // validation and partial-catalog policy, without a separate transport path.
    let value = match result {
        ServerResult::CustomResult(result) => result.0,
        result => serde_json::to_value(result).map_err(|_| ServiceError::UnexpectedResponse)?,
    };
    serde_json::from_value(value).map_err(|_| ServiceError::UnexpectedResponse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::skills::wire::SkillsListResult;
    use serde_json::json;

    #[test]
    fn native_skills_decode_keeps_metadata_and_future_cache_hints() {
        for cache_scope in ["private", "some-future-scope"] {
            let result = serde_json::from_value(json!({
                "resultType":"complete", "skills":[], "nextCursor":"page-2",
                "ttlMs":5000, "cacheScope":cache_scope, "_meta":{"retained":true}
            }))
            .expect("core SDK result");
            let decoded: SkillsListResult = decode(result).expect("Skills DTO");
            assert_eq!(decoded.cache_scope.as_deref(), Some(cache_scope));
            assert_eq!(decoded.next_cursor.as_deref(), Some("page-2"));
            assert_eq!(decoded.ttl_ms, Some(5000));
            assert_eq!(decoded.meta.as_ref().unwrap()["retained"], true);
        }
    }

    #[test]
    fn native_skills_decode_rejects_an_unrelated_result() {
        let result = serde_json::from_value(json!({"tools":[]})).unwrap();
        assert!(decode::<SkillsListResult>(result).is_err());
    }
}
