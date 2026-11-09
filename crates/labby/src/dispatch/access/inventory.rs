//! Shared inventory validation, deadlines, and read-only error contracts.

use std::time::Duration;

use axum::http::HeaderMap;
use labby_primitives::action::ParamSpec;
use labby_runtime::agent_error::{
    AgentErrorOrigin, AgentRecoveryAction, AgentRecoveryAdvice, AgentSameArgumentsRetry,
    AgentSideEffectRisk,
};
use serde_json::{Map, Value, json};

use crate::access::inventory::{
    AccessReadDeadline, InventoryError, InventoryPage, TeamInventoryInput, TeamInventoryKind,
};
use crate::access::{AccessRuntime, AccessStore};
use crate::dispatch::error::ToolError;

use super::{
    AccessDispatchContext, administration_authority_request, invalid, map_access_error,
    required_string,
};

pub(super) const PARAMS: &[ParamSpec] = &[
    super::string("team_id"),
    ParamSpec {
        name: "limit",
        ty: "integer",
        required: false,
        description: "Page size, default 50, range 1 to 100",
    },
    ParamSpec {
        name: "offset",
        ty: "integer",
        required: false,
        description: "Nonnegative offset, default 0",
    },
    ParamSpec {
        name: "snapshot_revision",
        ty: "integer",
        required: false,
        description: "Required for nonzero offset, from the first page",
    },
];

pub(crate) fn inventory_action(action: &str) -> bool {
    matches!(
        action,
        "access.team.member.list" | "access.team_project.list"
    )
}

pub(crate) fn read_deadline_from_headers(
    action: &str,
    headers: &HeaderMap,
) -> Result<Option<AccessReadDeadline>, ToolError> {
    if !inventory_action(action) {
        return Ok(None);
    }
    let name = "x-labby-access-read-budget-ms";
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(Some(AccessReadDeadline::server()));
    };
    if values.next().is_some() {
        return Err(invalid(name));
    }
    let text = value.to_str().map_err(|_| invalid(name))?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(name));
    }
    let millis: u64 = text.parse().map_err(|_| invalid(name))?;
    if millis == 0 {
        return Err(invalid(name));
    }
    Ok(Some(AccessReadDeadline::after(Duration::from_millis(
        millis.min(1_500),
    ))))
}

pub(crate) async fn inventory_store(
    runtime: &AccessRuntime,
    deadline: Option<AccessReadDeadline>,
) -> Result<AccessStore, ToolError> {
    let store = match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline.instant(), runtime.store())
            .await
            .map_err(|_| map_error(InventoryError::Timeout))?,
        None => runtime.store().await,
    };
    store.map_err(|error| {
        let error = crate::dispatch::access_errors::map_runtime_error("access", error);
        if deadline.is_some() {
            read_only(error)
        } else {
            error
        }
    })
}

pub(super) async fn dispatch(
    context: AccessDispatchContext,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let deadline = context
        .read_deadline
        .unwrap_or_else(AccessReadDeadline::server);
    let team_id = required_string(&params, "team_id")?;
    let page = page(&params)?;
    // This constructs the exact trusted request without any database access.
    let request = administration_authority_request(&context, action, &params)
        .await
        .map_err(read_only)?;
    context
        .store
        .list_team_inventory(TeamInventoryInput {
            request,
            team_id,
            kind: if action == "access.team.member.list" {
                TeamInventoryKind::Members
            } else {
                TeamInventoryKind::Assignments
            },
            page,
            deadline,
        })
        .await
        .map_err(map_error)
}

fn page(params: &Value) -> Result<InventoryPage, ToolError> {
    let integer = |name: &str, default: i64| -> Result<i64, ToolError> {
        params.get(name).map_or(Ok(default), |value| {
            value.as_i64().ok_or_else(|| invalid(name))
        })
    };
    let limit = integer("limit", 50)?;
    if !(1..=100).contains(&limit) {
        return Err(invalid("limit"));
    }
    let offset = integer("offset", 0)?;
    if offset < 0 {
        return Err(invalid("offset"));
    }
    let snapshot_revision = params
        .get("snapshot_revision")
        .map(|value| {
            value
                .as_i64()
                .filter(|value| *value >= 0)
                .ok_or_else(|| invalid("snapshot_revision"))
        })
        .transpose()?;
    if offset > 0 && snapshot_revision.is_none() {
        return Err(invalid("snapshot_revision"));
    }
    Ok(InventoryPage {
        limit: u32::try_from(limit).map_err(|_| invalid("limit"))?,
        offset,
        snapshot_revision,
    })
}

pub(crate) fn map_error(error: InventoryError) -> ToolError {
    use InventoryError as E;
    let (kind, message, extra, recovery) = match error {
        E::Store(error) => return read_only(map_access_error(error)),
        E::QueueSaturated => (
            "queue_saturated",
            "access inventory is busy",
            Map::new(),
            None,
        ),
        E::Timeout => (
            "timeout",
            "access inventory deadline expired",
            Map::new(),
            None,
        ),
        E::Cancelled => ("cancelled", "access inventory cancelled", Map::new(), None),
        E::CleanupFailed | E::WorkerUnavailable => (
            "service_unavailable",
            "access inventory is unavailable",
            Map::new(),
            None,
        ),
        E::RevisionConflict => (
            "conflict",
            "access inventory changed; restart at page zero",
            Map::from_iter([("existing_id".into(), "access_inventory".into())]),
            Some(advice(
                AgentRecoveryAction::Rediscover,
                "Restart this inventory at offset zero.",
            )),
        ),
        E::TooLarge { terminal } => (
            "response_too_large",
            "access inventory page exceeds its byte budget",
            Map::from_iter([("budget_terminal".into(), json!(terminal))]),
            Some(advice(
                if terminal {
                    AgentRecoveryAction::InspectAndEscalate
                } else {
                    AgentRecoveryAction::ReduceWork
                },
                if terminal {
                    "A single row exceeds the inventory budget. Ask the operator to inspect persisted metadata."
                } else {
                    "Retry with a smaller page limit."
                },
            )),
        ),
    };
    ToolError::contract(
        kind,
        message,
        extra,
        (kind == "response_too_large").then_some(AgentErrorOrigin::Budget),
        recovery,
        Some(AgentSideEffectRisk::NoneExpected),
    )
}

fn advice(action: AgentRecoveryAction, guidance: &str) -> AgentRecoveryAdvice {
    AgentRecoveryAdvice {
        action,
        same_arguments: AgentSameArgumentsRetry::Never,
        guidance: guidance.into(),
        retry_after_ms: None,
    }
}

fn read_only(error: ToolError) -> ToolError {
    ToolError::contract(
        error.kind(),
        error.user_message(),
        error
            .extra_fields()
            .as_object()
            .cloned()
            .unwrap_or_default(),
        None,
        None,
        Some(AgentSideEffectRisk::NoneExpected),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::test_support::sqlite_error as map_sqlite_error;
    use crate::access::{AddTeamMemberInput, AuthorityCeiling, CreateTeamInput, TeamRole};

    async fn fixture() -> (tempfile::TempDir, AccessDispatchContext) {
        let (directory, store, identity) = crate::access::test_support::fixture().await;
        (
            directory,
            AccessDispatchContext {
                store,
                identity,
                ceiling: AuthorityCeiling::trusted_local(),
                installation_id: "local-installation".into(),
                read_deadline: None,
                #[cfg(feature = "gateway")]
                gateway_manager: None,
            },
        )
    }

    async fn list(
        context: &AccessDispatchContext,
        action: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        super::super::dispatch(context.clone(), action, params).await
    }

    async fn revision(store: &AccessStore) -> i64 {
        crate::access::test_support::with_connection(store, |connection| {
            connection
                .query_row(
                    "SELECT global_revision FROM access_metadata WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)
        })
        .await
        .unwrap()
    }

    async fn seed_principal(context: &AccessDispatchContext, id: &str, role: TeamRole) {
        let principal = id.to_owned();
        crate::access::test_support::with_connection(&context.store, move |connection| {
            connection.execute(
                "INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at)
                 VALUES(?1,'bootstrap-local','user','active',1,1)",
                [&principal],
            ).map_err(map_sqlite_error)?;
            connection.execute(
                "INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,status,
                 verification_generation,link_generation,created_at,updated_at)
                 VALUES(?1,?2,'external','https://accounts.google.com',?2,'active',1,1,1,1)",
                rusqlite::params![format!("link-{principal}"), principal],
            ).map_err(map_sqlite_error)?;
            Ok(())
        }).await.unwrap();
        context
            .store
            .add_team_member(
                AddTeamMemberInput::new(
                    context.identity.clone(),
                    "bootstrap-initial-team",
                    id,
                    role,
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn inventory_persisted_states_epochs_binary_pages_and_revision_conflict() {
        let (_directory, context) = fixture().await;
        for id in ["z-member", "A-member", "a-member"] {
            seed_principal(&context, id, TeamRole::Member).await;
        }
        context.store.execute_test_statement("UPDATE team_memberships SET status='suspended',membership_epoch=7 WHERE principal_id='A-member'").await.unwrap();
        context.store.execute_test_statement("UPDATE team_memberships SET status='revoked',revoked_at=3,membership_epoch=8 WHERE principal_id='a-member'").await.unwrap();
        let before = revision(&context.store).await;
        let first = list(
            &context,
            "access.team.member.list",
            json!({"team_id":"bootstrap-initial-team","limit":1}),
        )
        .await
        .unwrap();
        assert_eq!(
            first["members"],
            json!([{"principal_id":"A-member","role":"member","status":"suspended","membership_epoch":7}])
        );
        assert!(first["team_membership_epoch"].as_u64().unwrap() > 1);
        assert_eq!(first["team_policy_epoch"], 1);
        assert_eq!(first["next_offset"], 1);
        let second = list(
            &context,
            "access.team.member.list",
            json!({
                "team_id":"bootstrap-initial-team",
                "limit":1,
                "offset":1,
                "snapshot_revision":first["global_revision"]
            }),
        )
        .await
        .unwrap();
        assert_eq!(second["members"][0]["principal_id"], "a-member");
        assert_eq!(second["members"][0]["status"], "revoked");
        assert_eq!(second["members"][0]["membership_epoch"], 8);
        assert_eq!(revision(&context.store).await, before);
        context
            .store
            .create_team(CreateTeamInput::new(context.identity.clone(), "new-team", "New").unwrap())
            .await
            .unwrap();
        let error = list(
            &context,
            "access.team.member.list",
            json!({
                "team_id":"bootstrap-initial-team",
                "offset":1,
                "snapshot_revision":first["global_revision"]
            }),
        )
        .await
        .unwrap_err()
        .to_agent_value();
        assert_eq!(error["kind"], "conflict");
        assert_eq!(error["existing_id"], "access_inventory");
        assert_eq!(error["side_effects"], "none_expected");
        assert_eq!(error["recovery"]["action"], "rediscover");
        assert_eq!(error["recovery"]["same_arguments"], "never");
    }

    #[tokio::test]
    async fn inventory_assignment_projection_includes_inactive_and_project_lifecycle() {
        let (_directory, context) = fixture().await;
        crate::access::test_support::with_connection(&context.store, |connection| {
            for (project, status, assignment_status, epoch) in [("project-a","suspended","active",2),("project-b","disabled","revoked",4),("project-c","active","suspended",6)] {
                connection.execute(
                    "INSERT INTO projects
                     VALUES(?1,'bootstrap-local',?1,?2,9,1,1)",
                    rusqlite::params![project,status],
                ).map_err(map_sqlite_error)?;
                connection.execute(
                    "INSERT INTO team_project_assignments
                     VALUES(?1,'bootstrap-local','bootstrap-initial-team',?1,'viewer',?2,?3,'bootstrap-owner',1,
                     1,CASE WHEN ?2='revoked' THEN 1 ELSE NULL END)",
                    rusqlite::params![project,assignment_status,epoch],
                ).map_err(map_sqlite_error)?;
            }
            Ok(())
        }).await.unwrap();
        let result = list(
            &context,
            "access.team_project.list",
            json!({"team_id":"bootstrap-initial-team","limit":2}),
        )
        .await
        .unwrap();
        assert_eq!(
            result["assignments"],
            json!([
                {"project_id":"project-a","role":"viewer","status":"active","assignment_epoch":2,"project_status":"suspended","project_policy_epoch":9},
                {"project_id":"project-b","role":"viewer","status":"revoked","assignment_epoch":4,"project_status":"disabled","project_policy_epoch":9}
            ])
        );
        assert_eq!(result["next_offset"], 2);
        let last = list(
            &context,
            "access.team_project.list",
            json!({
                "team_id":"bootstrap-initial-team",
                "limit":2,
                "offset":2,
                "snapshot_revision":result["global_revision"]
            }),
        )
        .await
        .unwrap();
        assert_eq!(last["assignments"][0]["status"], "suspended");
        assert_eq!(last["next_offset"], Value::Null);
    }

    #[tokio::test]
    async fn inventory_exact_authority_and_target_lifecycle_are_non_enumerating() {
        let (_directory, context) = fixture().await;
        for (id, role) in [
            ("member", TeamRole::Member),
            ("admin", TeamRole::Admin),
            ("owner", TeamRole::Owner),
        ] {
            seed_principal(&context, id, role).await;
        }
        context
            .store
            .create_team(
                CreateTeamInput::new(context.identity.clone(), "other-team", "Other").unwrap(),
            )
            .await
            .unwrap();
        // Use genuine host-verified identities linked to the persisted fixture.
        for action in ["access.team.member.list", "access.team_project.list"] {
            for id in ["member", "admin", "owner"] {
                let mut caller = context.clone();
                caller.identity = crate::access::test_support::browser(id);
                caller.ceiling = AuthorityCeiling::durable_execution();
                assert!(
                    list(&caller, action, json!({"team_id":"bootstrap-initial-team"}))
                        .await
                        .is_ok()
                );
                let wrong = list(&caller, action, json!({"team_id":"other-team"}))
                    .await
                    .unwrap_err()
                    .to_agent_value();
                let absent = list(&caller, action, json!({"team_id":"absent"}))
                    .await
                    .unwrap_err()
                    .to_agent_value();
                assert_eq!(wrong, absent);
                assert_eq!(wrong["kind"], "forbidden");
            }
            let mut no_ceiling = context.clone();
            no_ceiling.ceiling = AuthorityCeiling::from_auth_context(&labby_auth::AuthContext {
                sub: "ignored".into(),
                actor_key: None,
                scopes: vec![],
                issuer: "test".into(),
                via_session: false,
                csrf_token: None,
                email: None,
            });
            assert_eq!(
                list(
                    &no_ceiling,
                    action,
                    json!({"team_id":"bootstrap-initial-team"})
                )
                .await
                .unwrap_err()
                .kind(),
                "forbidden"
            );
        }
        context
            .store
            .execute_test_statement(
                "UPDATE groups SET status='suspended' WHERE group_id='bootstrap-initial-team'",
            )
            .await
            .unwrap();
        let mut member = context.clone();
        member.identity = crate::access::test_support::browser("member");
        for action in ["access.team.member.list", "access.team_project.list"] {
            assert_eq!(
                list(
                    &context,
                    action,
                    json!({"team_id":"bootstrap-initial-team"})
                )
                .await
                .unwrap()["team_status"],
                "suspended"
            );
            assert_eq!(
                list(&member, action, json!({"team_id":"bootstrap-initial-team"}))
                    .await
                    .unwrap_err()
                    .kind(),
                "forbidden"
            );
        }
        for status in ["deletion_pending", "deleted"] {
            crate::access::test_support::with_connection(&context.store, move |connection| {
                connection.execute(
                    "UPDATE groups SET status=?1,
                     deleted_at=CASE WHEN ?1='deleted' THEN 1 ELSE NULL END WHERE group_id='bootstrap-initial-team'",
                    [status],
                ).map_err(map_sqlite_error)?;
                Ok(())
            }).await.unwrap();
            for action in ["access.team.member.list", "access.team_project.list"] {
                assert_eq!(
                    list(
                        &context,
                        action,
                        json!({"team_id":"bootstrap-initial-team"})
                    )
                    .await
                    .unwrap_err()
                    .kind(),
                    "forbidden"
                );
            }
        }
    }

    #[tokio::test]
    async fn inventory_invalid_integers_and_nonzero_page_without_revision_are_rejected() {
        let (_directory, context) = fixture().await;
        for patch in [
            json!({"limit":0}),
            json!({"limit":101}),
            json!({"limit":1.5}),
            json!({"limit":"1"}),
            json!({"limit":null}),
            json!({"offset":-1}),
            json!({"offset":1}),
            json!({"offset":18_446_744_073_709_551_615_u64}),
            json!({"snapshot_revision":-1}),
            json!({"snapshot_revision":0.5}),
        ] {
            let mut params = json!({"team_id":"bootstrap-initial-team"});
            params
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert_eq!(
                list(&context, "access.team.member.list", params)
                    .await
                    .unwrap_err()
                    .kind(),
                "invalid_param"
            );
        }
    }

    #[tokio::test]
    async fn inventory_historical_long_ids_and_escaped_page_budget_are_terminal_at_one() {
        let (_directory, context) = fixture().await;
        crate::access::test_support::with_connection(&context.store, |connection| {
            // The schema predates product input bounds, so historical IDs can
            // exceed today's constructor limits without violating integrity.
            for (id,length) in [("old-a",11_000_i64),("old-b",11_000),("old-c",70_000)] {
                let principal = format!("{id}{}", "\\".repeat(usize::try_from(length).unwrap()));
                connection.execute(
                    "INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at)
                     VALUES(?1,'bootstrap-local','user','active',1,1)",
                    [&principal],
                ).map_err(map_sqlite_error)?;
                connection.execute(
                    "INSERT INTO team_memberships
                     VALUES(?1,'bootstrap-local','bootstrap-initial-team',?1,'member','revoked',3,
                     'bootstrap-owner',1,1,1)",
                    [&principal],
                ).map_err(map_sqlite_error)?;
            }
            Ok(())
        }).await.unwrap();
        let error = list(
            &context,
            "access.team.member.list",
            json!({"team_id":"bootstrap-initial-team"}),
        )
        .await
        .unwrap_err()
        .to_agent_value();
        assert_eq!(error["kind"], "response_too_large");
        assert_eq!(error["budget_terminal"], false);
        assert_eq!(error["side_effects"], "none_expected");
        assert_eq!(error["origin"], "budget");
        assert_eq!(error["recovery"]["action"], "reduce_work");
        let first = list(
            &context,
            "access.team.member.list",
            json!({"team_id":"bootstrap-initial-team","limit":1}),
        )
        .await
        .unwrap();
        assert_eq!(first["members"][0]["principal_id"], "bootstrap-owner");
        // The oversized lookahead row never poisons the preceding valid page.
        let second = list(
            &context,
            "access.team.member.list",
            json!({
                "team_id":"bootstrap-initial-team",
                "limit":1,
                "offset":2,
                "snapshot_revision":first["global_revision"]
            }),
        )
        .await
        .unwrap();
        assert!(serde_json::to_vec(&second).unwrap().len() <= crate::access::inventory::PAGE_BYTES);
        let terminal = list(
            &context,
            "access.team.member.list",
            json!({
                "team_id":"bootstrap-initial-team",
                "limit":1,
                "offset":3,
                "snapshot_revision":first["global_revision"]
            }),
        )
        .await
        .unwrap_err()
        .to_agent_value();
        assert_eq!(terminal["budget_terminal"], true);
        assert_eq!(terminal["recovery"]["action"], "inspect_and_escalate");
        assert!(!terminal.to_string().contains("old-c"));
    }

    #[tokio::test]
    async fn inventory_foreign_targets_and_former_or_inactive_members_are_denied() {
        let (_directory, context) = fixture().await;
        seed_principal(&context, "former", TeamRole::Member).await;
        context.store.execute_test_statement("INSERT INTO organizations VALUES('foreign','Foreign','active',1,1,1);
INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at) VALUES('foreign-owner','foreign','user','active',1,1);
INSERT INTO groups VALUES('foreign-team','foreign','team','Foreign','active',1,1,'foreign-owner',1,1,NULL);
INSERT INTO team_memberships VALUES('foreign-membership','foreign','foreign-team','foreign-owner','owner','active',1,'foreign-owner',1,1,NULL)").await.unwrap();
        let mut former = context.clone();
        former.identity = crate::access::test_support::browser("former");
        context.store.execute_test_statement("UPDATE team_memberships SET status='revoked',revoked_at=1,membership_epoch=membership_epoch+1 WHERE principal_id='former'").await.unwrap();
        for action in ["access.team.member.list", "access.team_project.list"] {
            let foreign = list(&context, action, json!({"team_id":"foreign-team"}))
                .await
                .unwrap_err()
                .to_agent_value();
            let absent = list(&context, action, json!({"team_id":"absent"}))
                .await
                .unwrap_err()
                .to_agent_value();
            assert_eq!(foreign, absent);
            assert_eq!(
                list(&former, action, json!({"team_id":"bootstrap-initial-team"}))
                    .await
                    .unwrap_err()
                    .kind(),
                "forbidden"
            );
        }
        context
            .store
            .execute_test_statement(
                "UPDATE principals SET status='suspended' WHERE principal_id='bootstrap-owner'",
            )
            .await
            .unwrap();
        assert_eq!(
            list(
                &context,
                "access.team.member.list",
                json!({"team_id":"bootstrap-initial-team"})
            )
            .await
            .unwrap_err()
            .kind(),
            "forbidden"
        );
    }

    #[tokio::test]
    async fn inventory_authority_revision_and_rows_share_snapshot_during_concurrent_write() {
        let (directory, context) = fixture().await;
        seed_principal(&context, "reader", TeamRole::Member).await;
        let mut reader = context.clone();
        reader.identity = crate::access::test_support::browser("reader");
        reader.ceiling = AuthorityCeiling::durable_execution();
        let before = revision(&context.store).await;
        let writer = AccessStore::open_existing_current(directory.path().join("access.db"))
            .await
            .unwrap();
        let (started, observed_start) = tokio::sync::oneshot::channel();
        let (release, wait_release) = std::sync::mpsc::sync_channel(0);
        crate::access::test_support::with_connection(&context.store, move |connection| {
            let mut started = Some(started);
            connection
                .authorizer(Some(move |event: rusqlite::hooks::AuthContext<'_>| {
                    if matches!(
                        event.action,
                        rusqlite::hooks::AuthAction::Read {
                            table_name: "team_memberships",
                            ..
                        }
                    ) {
                        if let Some(started) = started.take() {
                            started.send(()).unwrap();
                            wait_release.recv_timeout(Duration::from_secs(2)).unwrap();
                        }
                    }
                    rusqlite::hooks::Authorization::Allow
                }))
                .map_err(map_sqlite_error)
        })
        .await
        .unwrap();
        let in_flight = reader.clone();
        let request = tokio::spawn(async move {
            list(
                &in_flight,
                "access.team.member.list",
                json!({"team_id":"bootstrap-initial-team"}),
            )
            .await
        });
        observed_start.await.unwrap();
        writer
            .suspend_team_member(
                crate::access::TeamMembershipInput::new(
                    context.identity.clone(),
                    "bootstrap-initial-team",
                    "reader",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        release.send(()).unwrap();
        let response = request.await.unwrap().unwrap();
        assert_eq!(response["global_revision"], before);
        let rows = response["members"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row["principal_id"] == "reader")
                .unwrap()["membership_epoch"],
            1
        );
        crate::access::test_support::with_connection(&context.store, |connection| {
            connection
                .authorizer(
                    None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
                )
                .map_err(map_sqlite_error)
        })
        .await
        .unwrap();
        assert_eq!(
            list(
                &reader,
                "access.team.member.list",
                json!({"team_id":"bootstrap-initial-team"})
            )
            .await
            .unwrap_err()
            .kind(),
            "forbidden"
        );
    }

    #[tokio::test]
    async fn inventory_current_actor_and_historical_assignment_values_are_bounded() {
        let (_directory, context) = fixture().await;
        crate::access::test_support::with_connection(&context.store, |connection| {
            let principal = format!("A{}","historical".repeat(8_000));
            connection.execute(
                "INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at)
                 VALUES(?1,'bootstrap-local','user','active',1,1)",
                [&principal],
            ).map_err(map_sqlite_error)?;
            connection.execute(
                "INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,status,
                 verification_generation,link_generation,created_at,updated_at)
                 VALUES('old-link',?1,'external','https://accounts.google.com','old-actor','active',1,1,1,
                 1)",
                [&principal],
            ).map_err(map_sqlite_error)?;
            connection.execute(
                "INSERT INTO team_memberships
                 VALUES('old-membership','bootstrap-local','bootstrap-initial-team',?1,'member','active',1,
                 'bootstrap-owner',1,1,NULL)",
                [&principal],
            ).map_err(map_sqlite_error)?;
            // Raw text fits the page cap, while JSON escaping exceeds it.
            let project = "\"".repeat(31_000);
            connection.execute(
                "INSERT INTO projects
                 VALUES(?1,'bootstrap-local','Old','active',1,1,1)",
                [&project],
            ).map_err(map_sqlite_error)?;
            connection.execute(
                "INSERT INTO team_project_assignments
                 VALUES('old-assignment','bootstrap-local','bootstrap-initial-team',?1,'viewer','revoked',3,
                 'bootstrap-owner',1,1,1)",
                [&project],
            ).map_err(map_sqlite_error)?;
            Ok(())
        }).await.unwrap();
        let mut actor = context.clone();
        actor.identity = crate::access::test_support::browser("old-actor");
        actor.ceiling = AuthorityCeiling::durable_execution();
        for action in ["access.team.member.list", "access.team_project.list"] {
            let error = list(
                &context,
                action,
                json!({"team_id":"bootstrap-initial-team","limit":1}),
            )
            .await
            .unwrap_err()
            .to_agent_value();
            assert_eq!(error["kind"], "response_too_large", "{error}");
            assert_eq!(error["budget_terminal"], true);
            assert!(!error.to_string().contains("historical"));
        }
        // Preserve the evaluator's existing canonical PrincipalId constraint.
        // An oversized historical actor fails before any inventory is emitted.
        let error = list(
            &actor,
            "access.team.member.list",
            json!({"team_id":"bootstrap-initial-team","limit":1}),
        )
        .await
        .unwrap_err()
        .to_agent_value();
        assert_eq!(error["kind"], "service_unavailable");
        assert_eq!(error["side_effects"], "none_expected");
        assert!(!error.to_string().contains("historical"));
    }

    #[tokio::test]
    async fn inventory_historical_team_id_uses_existing_authority_identifier_contract() {
        let (_directory, context) = fixture().await;
        let team_id = format!("historical-{}", "x".repeat(200));
        let stored_id = team_id.clone();
        crate::access::test_support::with_connection(&context.store,move |connection| {
            connection.execute(
                "INSERT INTO groups VALUES(?1,'bootstrap-local','team','Old','active',1,1,'bootstrap-owner',1,1,NULL)",
                [&stored_id],
            ).map_err(map_sqlite_error)?;
            connection.execute(
                "INSERT INTO team_memberships VALUES('old-team-membership','bootstrap-local',?1,'bootstrap-owner','owner','active',1,'bootstrap-owner',1,1,NULL)",
                [&stored_id],
            ).map_err(map_sqlite_error)?;
            Ok(())
        }).await.unwrap();
        for action in ["access.team.member.list", "access.team_project.list"] {
            let value = list(&context, action, json!({"team_id":team_id}))
                .await
                .unwrap();
            assert_eq!(value["team_id"], team_id);
        }
    }

    #[test]
    fn inventory_header_is_checked_shortening_only_and_not_applied_to_writes() {
        for bad in ["0", "-1", "1.5", "1e3", " 1", "+1", "18446744073709551616"] {
            let mut headers = HeaderMap::new();
            headers.insert("x-labby-access-read-budget-ms", bad.parse().unwrap());
            assert_eq!(
                read_deadline_from_headers("access.team.member.list", &headers)
                    .unwrap_err()
                    .kind(),
                "invalid_param"
            );
            assert!(
                read_deadline_from_headers("access.team.create", &headers)
                    .unwrap()
                    .is_none()
            );
        }
        let mut headers = HeaderMap::new();
        headers.insert("x-labby-access-read-budget-ms", "99999".parse().unwrap());
        assert!(
            read_deadline_from_headers("access.team.member.list", &headers)
                .unwrap()
                .unwrap()
                .remaining()
                <= Duration::from_millis(1500)
        );
        headers.insert("x-labby-access-read-budget-ms", "10".parse().unwrap());
        assert!(
            read_deadline_from_headers("access.team.member.list", &headers)
                .unwrap()
                .unwrap()
                .remaining()
                <= Duration::from_millis(10)
        );
        headers.append("x-labby-access-read-budget-ms", "10".parse().unwrap());
        assert!(read_deadline_from_headers("access.team.member.list", &headers).is_err());
    }
}
