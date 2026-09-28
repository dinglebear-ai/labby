//! Request-local telemetry identity, independent of upstream credential scope.
//! Callers must supply a redacted tag from verified identity, never wire metadata.
use std::future::Future;

tokio::task_local! {
    static ACTOR: Option<String>;
    static ATTRIBUTION: Option<UsageAttribution>;
}

/// Inbound identity is verified; client labels are bounded, self-reported MCP
/// initialize metadata. Agent/task IDs are supplied only by trusted execution.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct UsageAttribution {
    pub inbound_actor: Option<String>,
    pub actor_kind: Option<String>,
    pub surface: Option<String>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub harness_id: Option<String>,
    pub upstream_subject_tag: Option<String>,
}
impl UsageAttribution {
    pub fn inbound(actor: Option<String>, surface: &str, client: Option<(&str, &str)>) -> Self {
        let label = |value: &str| {
            value
                .chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect::<String>()
        };
        Self {
            inbound_actor: actor,
            actor_kind: Some(
                if client.is_some() {
                    "client"
                } else {
                    "subject"
                }
                .into(),
            ),
            surface: Some(surface.into()),
            client_name: client.map(|(name, _)| label(name)),
            client_version: client.map(|(_, version)| label(version)),
            ..Self::default()
        }
    }
}
pub async fn scope_attributed<T>(
    attribution: UsageAttribution,
    future: impl Future<Output = T>,
) -> T {
    ACTOR
        .scope(
            attribution.inbound_actor.clone(),
            ATTRIBUTION.scope(Some(attribution), future),
        )
        .await
}
pub fn attribution() -> Option<UsageAttribution> {
    ATTRIBUTION.try_with(Clone::clone).ok().flatten()
}

pub async fn scope<T>(actor: Option<String>, future: impl Future<Output = T>) -> T {
    ACTOR.scope(actor, future).await
}

pub fn current() -> Option<String> {
    ACTOR.try_with(Clone::clone).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn initialized_client_metadata_is_bounded_and_scoped() {
        let value = UsageAttribution::inbound(
            Some("sub:verified".into()),
            "mcp",
            Some((&"x".repeat(1000), "1\n2")),
        );
        assert_eq!(value.client_name.as_ref().unwrap().len(), 128);
        assert_eq!(value.client_version.as_deref(), Some("12"));
        scope_attributed(value.clone(), async {
            tokio::task::yield_now().await;
            assert_eq!(attribution(), Some(value));
            assert_eq!(current().as_deref(), Some("sub:verified"));
        })
        .await;
        assert_eq!(attribution(), None);
    }
    #[tokio::test]
    async fn concurrent_verified_actors_are_isolated_and_do_not_escape_scope() {
        let (first, second) = tokio::join!(
            scope(Some("sub:first".into()), async {
                tokio::task::yield_now().await;
                current()
            }),
            scope(Some("sub:second".into()), async {
                tokio::task::yield_now().await;
                current()
            })
        );
        assert_eq!(first.as_deref(), Some("sub:first"));
        assert_eq!(second.as_deref(), Some("sub:second"));
        assert_eq!(current(), None);
    }
}
