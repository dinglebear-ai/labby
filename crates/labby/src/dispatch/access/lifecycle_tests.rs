use super::*;
use crate::access::{AccessStore, CreateTeamInput};

async fn call(
    store: &AccessStore,
    identity: &VerifiedIdentity,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    dispatch(
        AccessDispatchContext {
            store: store.clone(),
            identity: identity.clone(),
            ceiling: AuthorityCeiling::trusted_local(),
            read_deadline: None,
            installation_id: "local-installation".into(),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
        },
        action,
        params,
    )
    .await
}

async fn prepare() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
    let fixture = crate::access::test_support::fixture().await;
    fixture
        .1
        .create_team(CreateTeamInput::new(fixture.2.clone(), "ordinary", "Ordinary").unwrap())
        .await
        .unwrap();
    fixture
}

#[tokio::test]
async fn lifecycle_activation_restores_suspended_membership_through_shared_dispatch() {
    let (_directory, store, owner) = prepare().await;
    store
        .suspend_team_member(
            TeamMembershipInput::new(owner.clone(), "ordinary", "bootstrap-owner").unwrap(),
        )
        .await
        .unwrap_err();
    // A second active owner keeps the existing final-owner protection meaningful.
    crate::access::test_support::with_connection(&store, |connection| {
        connection.execute("INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at) VALUES('second','bootstrap-local','user','active',1,1)", []).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO team_memberships VALUES('second-owner','bootstrap-local','ordinary','second','owner','active',1,'bootstrap-owner',1,1,NULL)", []).map_err(crate::access::test_support::sqlite_error)?;
        Ok(())
    }).await.unwrap();
    store
        .suspend_team_member(
            TeamMembershipInput::new(owner.clone(), "ordinary", "bootstrap-owner").unwrap(),
        )
        .await
        .unwrap();
    let result = call(
        &store,
        &owner,
        "access.team.member.activate",
        json!({"team_id":"ordinary","principal_id":"bootstrap-owner","role":"owner"}),
    )
    .await
    .unwrap();
    assert_eq!(
        result,
        json!({"team_id":"ordinary","principal_id":"bootstrap-owner","role":"owner","status":"active","membership_epoch":3})
    );
    let roster = call(
        &store,
        &owner,
        "access.team.member.list",
        json!({"team_id":"ordinary","limit":10}),
    )
    .await
    .unwrap();
    assert_eq!(roster["members"][0]["status"], "active");
}

#[tokio::test]
async fn lifecycle_unassign_revokes_assignment_through_shared_dispatch() {
    let (_directory, store, owner) = prepare().await;
    call(
        &store,
        &owner,
        "access.team_project.assign",
        json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"admin"}),
    )
    .await
    .unwrap();
    let result = call(
        &store,
        &owner,
        "access.team_project.unassign",
        json!({"team_id":"ordinary","project_id":"bootstrap-default"}),
    )
    .await
    .unwrap();
    assert_eq!(
        result,
        json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"admin","status":"revoked","assignment_epoch":2})
    );
    let inventory = call(
        &store,
        &owner,
        "access.team_project.list",
        json!({"team_id":"ordinary","limit":10}),
    )
    .await
    .unwrap();
    assert_eq!(inventory["assignments"][0]["status"], "revoked");
}

#[tokio::test]
async fn lifecycle_action_metadata_and_schema_are_canonical() {
    for (name, capability, fields) in [
        (
            "access.team.member.activate",
            Capability::MembershipManage,
            vec!["team_id", "principal_id", "role"],
        ),
        (
            "access.team_project.unassign",
            Capability::ScopeManage,
            vec!["team_id", "project_id"],
        ),
    ] {
        let spec = ACTIONS
            .iter()
            .find(|spec| spec.name == name)
            .expect("lifecycle action missing from catalog");
        assert!(!spec.requires_admin);
        assert!(!spec.destructive);
        assert_eq!(required_capability(name), Some(capability));
        assert_eq!(
            spec.params.iter().map(|p| p.name).collect::<Vec<_>>(),
            fields
        );
        assert!(spec.params.iter().all(|p| p.required && p.ty == "string"));
        let schema = dispatch_unbound("schema", json!({"action":name}))
            .await
            .unwrap();
        assert_eq!(
            schema["params"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            fields
        );
        let help = dispatch_unbound("help", json!({})).await.unwrap();
        assert!(
            help["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == name)
        );
    }
}

async fn seed(store: &AccessStore, id: &str, role: &str) -> VerifiedIdentity {
    let id = id.to_owned();
    let role = role.to_owned();
    let identity = crate::access::test_support::browser(&id);
    crate::access::test_support::with_connection(store, move |connection| {
        connection.execute("INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at) VALUES(?1,'bootstrap-local','user','active',1,1)", [&id]).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'external','https://accounts.google.com',?2,'active',1,1,1,1)", rusqlite::params![format!("link-{id}"),id]).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO team_memberships VALUES(?1,'bootstrap-local','ordinary',?2,?3,'active',1,'bootstrap-owner',1,1,NULL)", rusqlite::params![format!("historical-{id}"),id,role]).map_err(crate::access::test_support::sqlite_error)?;
        Ok(())
    }).await.unwrap();
    identity
}

async fn statement(store: &AccessStore, sql: &str) {
    let sql = sql.to_owned();
    crate::access::test_support::with_connection(store, move |connection| {
        connection
            .execute_batch(&sql)
            .map_err(crate::access::test_support::sqlite_error)
    })
    .await
    .unwrap();
}

// Capture every persisted column of the authority, lifecycle and event tables.
// Equality catches partial writes even when a public projection hides the row.
async fn persisted(store: &AccessStore) -> Vec<Vec<Vec<String>>> {
    crate::access::test_support::with_connection(store, |connection| {
        [
            "organizations",
            "principals",
            "principal_links",
            "groups",
            "team_memberships",
            "projects",
            "project_memberships",
            "team_project_assignments",
            "platform_administrators",
            "access_metadata",
            "access_audit",
            "authority_projection_outbox",
            "authority_outbox_sequences",
        ]
        .into_iter()
        .map(|table| {
            let mut query = connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .map_err(crate::access::test_support::sqlite_error)?;
            let width = query.column_count();
            query
                .query_map([], |row| {
                    (0..width)
                        .map(|column| row.get_ref(column).map(|value| format!("{value:?}")))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(crate::access::test_support::sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(crate::access::test_support::sqlite_error)
        })
        .collect()
    })
    .await
    .unwrap()
}

async fn epochs(store: &AccessStore) -> [i64; 5] {
    crate::access::test_support::with_connection(store, |connection| {
        connection.query_row("SELECT global_revision,(SELECT count(*) FROM access_audit),(SELECT count(*) FROM authority_projection_outbox),(SELECT membership_epoch FROM groups WHERE group_id='ordinary'),(SELECT project_policy_epoch FROM projects WHERE project_id='bootstrap-default') FROM access_metadata", [], |row| Ok([row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?])).map_err(crate::access::test_support::sqlite_error)
    }).await.unwrap()
}

fn activation(team: &str, target: &str, role: &str) -> Value {
    json!({"team_id":team,"principal_id":target,"role":role})
}
fn assignment() -> Value {
    json!({"team_id":"ordinary","project_id":"bootstrap-default"})
}

#[tokio::test]
async fn lifecycle_activation_preserves_history_and_advances_only_owning_epochs() {
    for status in ["suspended", "revoked"] {
        let (_directory, store, owner) = prepare().await;
        seed(&store, "target", "member").await;
        let input = TeamMembershipInput::new(owner.clone(), "ordinary", "target").unwrap();
        if status == "revoked" {
            store.remove_team_member(input).await.unwrap();
        } else {
            store.suspend_team_member(input).await.unwrap();
        }
        if status == "suspended" {
            call(
                &store,
                &owner,
                "access.team.member.role.set",
                activation("ordinary", "target", "admin"),
            )
            .await
            .unwrap();
            let roster = call(
                &store,
                &owner,
                "access.team.member.list",
                json!({"team_id":"ordinary","limit":10}),
            )
            .await
            .unwrap();
            assert_eq!(
                roster["members"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["principal_id"] == "target")
                    .unwrap()["status"],
                "suspended"
            );
        }
        let before = persisted(&store).await;
        assert_eq!(
            call(
                &store,
                &owner,
                "access.team.member.add",
                activation("ordinary", "target", "member")
            )
            .await
            .unwrap_err()
            .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
        let before = epochs(&store).await;
        let roster = call(
            &store,
            &owner,
            "access.team.member.list",
            json!({"team_id":"ordinary","limit":10}),
        )
        .await
        .unwrap();
        let previous = roster["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["principal_id"] == "target")
            .unwrap();
        let result = call(
            &store,
            &owner,
            "access.team.member.activate",
            activation("ordinary", "target", "admin"),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "active");
        assert_eq!(result["role"], "admin");
        assert_eq!(
            result["membership_epoch"].as_u64().unwrap(),
            previous["membership_epoch"].as_u64().unwrap() + 1
        );
        let after = epochs(&store).await;
        assert_eq!(
            after,
            [
                before[0] + 1,
                before[1] + 1,
                before[2] + 1,
                before[3] + 1,
                before[4]
            ]
        );
        let row = crate::access::test_support::with_connection(&store, |connection| {
            connection.query_row("SELECT membership_id,created_by,created_at,revoked_at,(SELECT action FROM access_audit ORDER BY rowid DESC LIMIT 1) FROM team_memberships WHERE principal_id='target'", [], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,Option<i64>>(3)?,row.get::<_,String>(4)?))).map_err(crate::access::test_support::sqlite_error)
        }).await.unwrap();
        assert_eq!(
            row,
            (
                "historical-target".into(),
                "bootstrap-owner".into(),
                1,
                None,
                "access.team_member.activate".into()
            )
        );
        let before = persisted(&store).await;
        assert_eq!(
            call(
                &store,
                &owner,
                "access.team.member.activate",
                activation("ordinary", "target", "admin")
            )
            .await
            .unwrap_err()
            .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
    }
}

#[tokio::test]
async fn lifecycle_activation_enforces_current_and_requested_owner_authority() {
    for (actor_role, previous, requested, allowed) in [
        ("member", "member", "member", false),
        ("admin", "member", "admin", true),
        ("admin", "owner", "member", false),
        ("admin", "member", "owner", false),
        ("owner", "owner", "member", true),
        ("owner", "member", "owner", true),
    ] {
        let (_directory, store, owner) = prepare().await;
        let actor = seed(&store, "actor", actor_role).await;
        seed(&store, "target", previous).await;
        store
            .remove_team_member(TeamMembershipInput::new(owner, "ordinary", "target").unwrap())
            .await
            .unwrap();
        let before = persisted(&store).await;
        let result = call(
            &store,
            &actor,
            "access.team.member.activate",
            activation("ordinary", "target", requested),
        )
        .await;
        if allowed {
            assert_eq!(result.unwrap()["role"], requested);
        } else {
            assert_eq!(result.unwrap_err().kind(), "forbidden");
            assert_eq!(persisted(&store).await, before);
        }
    }
}

#[tokio::test]
async fn lifecycle_activation_rejects_initial_team_readmission_and_inactive_targets() {
    let (_directory, store, owner) = prepare().await;
    seed(&store, "target", "member").await;
    store
        .add_team_member(
            AddTeamMemberInput::new(
                owner.clone(),
                "bootstrap-initial-team",
                "target",
                TeamRole::Member,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .remove_team_member(
            TeamMembershipInput::new(owner.clone(), "bootstrap-initial-team", "target").unwrap(),
        )
        .await
        .unwrap();
    let before = persisted(&store).await;
    assert_eq!(
        call(
            &store,
            &owner,
            "access.team.member.activate",
            activation("bootstrap-initial-team", "target", "owner")
        )
        .await
        .unwrap_err()
        .kind(),
        "forbidden"
    );
    assert_eq!(persisted(&store).await, before);
    store
        .suspend_team_member(TeamMembershipInput::new(owner.clone(), "ordinary", "target").unwrap())
        .await
        .unwrap();
    for state in ["suspended", "disabled"] {
        statement(
            &store,
            &format!("UPDATE principals SET status='{state}' WHERE principal_id='target'"),
        )
        .await;
        let before = persisted(&store).await;
        assert_eq!(
            call(
                &store,
                &owner,
                "access.team.member.activate",
                activation("ordinary", "target", "member")
            )
            .await
            .unwrap_err()
            .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
    }
    statement(
        &store,
        "UPDATE principals SET status='active' WHERE principal_id='target'",
    )
    .await;
    // Suspended Initial Team membership is a separate explicit restoration case.
    statement(&store,"UPDATE team_memberships SET status='suspended',revoked_at=NULL WHERE team_id='bootstrap-initial-team' AND principal_id='target'").await;
    assert_eq!(
        call(
            &store,
            &owner,
            "access.team.member.activate",
            activation("bootstrap-initial-team", "target", "member")
        )
        .await
        .unwrap()["status"],
        "active"
    );
}

#[tokio::test]
async fn lifecycle_mutations_reject_foreign_missing_lifecycle_and_current_identity_targets() {
    let (_directory, store, owner) = prepare().await;
    seed(&store, "target", "member").await;
    store
        .suspend_team_member(TeamMembershipInput::new(owner.clone(), "ordinary", "target").unwrap())
        .await
        .unwrap();
    call(
        &store,
        &owner,
        "access.team_project.assign",
        json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"member"}),
    )
    .await
    .unwrap();
    for sql in [
        "UPDATE principal_links SET status='revoked' WHERE principal_id='bootstrap-owner'",
        "UPDATE principals SET status='disabled' WHERE principal_id='bootstrap-owner'",
        "UPDATE groups SET status='deletion_pending' WHERE group_id='ordinary'",
        "UPDATE groups SET status='deleted',deleted_at=1 WHERE group_id='ordinary'",
    ] {
        statement(&store, sql).await;
        let before = persisted(&store).await;
        for (action, params) in [
            (
                "access.team.member.activate",
                activation("ordinary", "target", "member"),
            ),
            ("access.team_project.unassign", assignment()),
        ] {
            assert_eq!(
                call(&store, &owner, action, params)
                    .await
                    .unwrap_err()
                    .kind(),
                "forbidden"
            );
            assert_eq!(persisted(&store).await, before);
        }
        statement(&store,"UPDATE principal_links SET status='active' WHERE principal_id='bootstrap-owner'; UPDATE principals SET status='active' WHERE principal_id='bootstrap-owner'; UPDATE groups SET status='active',deleted_at=NULL WHERE group_id='ordinary'").await;
    }
    for (action, params) in [
        (
            "access.team.member.activate",
            activation("missing", "target", "member"),
        ),
        (
            "access.team.member.activate",
            activation("ordinary", "absent", "member"),
        ),
        (
            "access.team_project.unassign",
            json!({"team_id":"missing","project_id":"bootstrap-default"}),
        ),
        (
            "access.team_project.unassign",
            json!({"team_id":"ordinary","project_id":"missing"}),
        ),
    ] {
        let before = persisted(&store).await;
        assert_eq!(
            call(&store, &owner, action, params)
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
    }
    for state in ["suspended", "disabled"] {
        statement(
            &store,
            &format!("UPDATE projects SET status='{state}' WHERE project_id='bootstrap-default'"),
        )
        .await;
        let before = persisted(&store).await;
        assert_eq!(
            call(&store, &owner, "access.team_project.unassign", assignment())
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
    }
}

#[tokio::test]
async fn lifecycle_mutations_rollback_all_authority_state_on_audit_or_outbox_failure() {
    for (table, action, params) in [
        (
            "access_audit",
            "access.team.member.activate",
            activation("ordinary", "target", "admin"),
        ),
        (
            "authority_projection_outbox",
            "access.team.member.activate",
            activation("ordinary", "target", "admin"),
        ),
        ("access_audit", "access.team_project.unassign", assignment()),
        (
            "authority_projection_outbox",
            "access.team_project.unassign",
            assignment(),
        ),
    ] {
        let (_directory, store, owner) = prepare().await;
        seed(&store, "target", "member").await;
        store
            .suspend_team_member(
                TeamMembershipInput::new(owner.clone(), "ordinary", "target").unwrap(),
            )
            .await
            .unwrap();
        call(
            &store,
            &owner,
            "access.team_project.assign",
            json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"admin"}),
        )
        .await
        .unwrap();
        statement(&store,&format!("CREATE TEMP TRIGGER reject_lifecycle BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'injected lifecycle failure'); END")).await;
        let before = persisted(&store).await;
        let error = call(&store, &owner, action, params).await.unwrap_err();
        assert_eq!(error.kind(), "service_unavailable");
        assert!(!error.to_string().contains("injected"));
        assert_eq!(persisted(&store).await, before);
    }
}

#[tokio::test]
async fn lifecycle_unassign_checks_dual_authority_and_allows_loss_of_sole_assignment() {
    let (_directory, store, owner) = prepare().await;
    let actor = seed(&store, "actor", "admin").await;
    seed(&store, "member", "member").await;
    let unrelated = seed(&store, "unrelated", "admin").await;
    statement(
        &store,
        "DELETE FROM team_memberships WHERE principal_id='unrelated'",
    )
    .await;
    for role in ["viewer", "member", "admin"] {
        call(
            &store,
            &owner,
            "access.team_project.assign",
            json!({"team_id":"ordinary","project_id":"bootstrap-default","role":role}),
        )
        .await
        .unwrap();
        let before = persisted(&store).await;
        for identity in [&unrelated, &crate::access::test_support::browser("member")] {
            assert_eq!(
                call(
                    &store,
                    identity,
                    "access.team_project.unassign",
                    assignment()
                )
                .await
                .unwrap_err()
                .kind(),
                "forbidden"
            );
            assert_eq!(persisted(&store).await, before);
        }
        if role != "admin" {
            assert_eq!(
                call(&store, &actor, "access.team_project.unassign", assignment())
                    .await
                    .unwrap_err()
                    .kind(),
                "forbidden"
            );
            assert_eq!(persisted(&store).await, before);
        }
    }
    let before = epochs(&store).await;
    let effective = call(&store, &actor, "access.project.effective.list", json!({}))
        .await
        .unwrap();
    assert_eq!(effective["projects"][0]["role"], "admin");
    let result = call(&store, &actor, "access.team_project.unassign", assignment())
        .await
        .unwrap();
    assert_eq!(result["status"], "revoked");
    let after = epochs(&store).await;
    assert_eq!(
        after,
        [
            before[0] + 1,
            before[1] + 1,
            before[2] + 1,
            before[3],
            before[4] + 1
        ]
    );
    assert_eq!(
        call(&store, &actor, "access.project.effective.list", json!({}))
            .await
            .unwrap()["projects"],
        json!([])
    );
    assert!(
        store
            .list_managed_projects(actor.clone())
            .await
            .unwrap()
            .is_empty()
    );
    let before = persisted(&store).await;
    assert_eq!(
        call(&store, &actor, "access.team_project.unassign", assignment())
            .await
            .unwrap_err()
            .kind(),
        "forbidden"
    );
    assert_eq!(
        call(
            &store,
            &actor,
            "access.team_project.assign",
            json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"admin"})
        )
        .await
        .unwrap_err()
        .kind(),
        "forbidden"
    );
    assert_eq!(persisted(&store).await, before);
    let inventory = call(
        &store,
        &owner,
        "access.team_project.list",
        json!({"team_id":"ordinary","limit":10}),
    )
    .await
    .unwrap();
    assert_eq!(inventory["assignments"][0]["status"], "revoked");
    let epoch = result["assignment_epoch"].as_u64().unwrap();
    let recovered = call(
        &store,
        &owner,
        "access.team_project.assign",
        json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"owner"}),
    )
    .await
    .unwrap();
    assert_eq!(recovered["assignment_epoch"].as_u64().unwrap(), epoch + 1);
    let row = crate::access::test_support::with_connection(&store,|connection| {
        connection.query_row("SELECT assignment_id,created_by,revoked_at,(SELECT role FROM project_memberships WHERE principal_id='bootstrap-owner'),(SELECT status FROM projects WHERE project_id='bootstrap-default') FROM team_project_assignments WHERE team_id='ordinary'",[],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<i64>>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?))).map_err(crate::access::test_support::sqlite_error)
    }).await.unwrap();
    assert_eq!(
        row,
        (
            "team-project-ordinary-bootstrap-default".into(),
            "bootstrap-owner".into(),
            None,
            "owner".into(),
            "active".into()
        )
    );
}

#[tokio::test]
async fn lifecycle_activation_active_sole_owner_is_unavailable() {
    let (_directory, store, owner) = prepare().await;
    let before = persisted(&store).await;
    let error = call(
        &store,
        &owner,
        "access.team.member.activate",
        activation("ordinary", "bootstrap-owner", "member"),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), "forbidden");
    assert_eq!(persisted(&store).await, before);
}

#[tokio::test]
async fn lifecycle_initial_team_activation_cannot_restore_allowlist_revocation() {
    let (_directory, store, owner) = prepare().await;
    let target = crate::access::test_support::browser("allowlisted-target");
    store
        .provision_allowlisted(
            target.clone(),
            crate::access::AllowedUserRole::Admin,
            crate::access::AllowlistAdmission::ConfiguredAdminEmail,
        )
        .await
        .unwrap();
    let principal = crate::access::test_support::with_connection(&store, |connection| {
        connection
            .query_row(
                "SELECT principal_id FROM principal_links WHERE subject='allowlisted-target'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(crate::access::test_support::sqlite_error)
    })
    .await
    .unwrap();
    assert!(
        store
            .revoke_allowlisted(target.clone(), "admin-fingerprint".into())
            .await
            .unwrap()
            .revoked_anything()
    );
    let before = persisted(&store).await;
    assert_eq!(
        call(
            &store,
            &owner,
            "access.team.member.activate",
            activation("bootstrap-initial-team", &principal, "admin")
        )
        .await
        .unwrap_err()
        .kind(),
        "forbidden"
    );
    assert_eq!(persisted(&store).await, before);
    assert_eq!(
        store
            .provision_allowlisted(
                target,
                crate::access::AllowedUserRole::Admin,
                crate::access::AllowlistAdmission::ConfiguredAdminEmail
            )
            .await
            .unwrap(),
        crate::access::TeamMemberProvisionOutcome::AlreadyActive
    );
    assert_eq!(persisted(&store).await, before);
}

#[tokio::test]
async fn lifecycle_mutations_enforce_transport_ceiling_and_current_transaction_authority() {
    let (_directory, store, owner) = prepare().await;
    let actor = seed(&store, "actor", "admin").await;
    seed(&store, "target", "member").await;
    store
        .suspend_team_member(TeamMembershipInput::new(owner.clone(), "ordinary", "target").unwrap())
        .await
        .unwrap();
    call(
        &store,
        &owner,
        "access.team_project.assign",
        json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"admin"}),
    )
    .await
    .unwrap();
    for (action, params) in [
        (
            "access.team.member.activate",
            activation("ordinary", "target", "member"),
        ),
        ("access.team_project.unassign", assignment()),
    ] {
        let mut context = AccessDispatchContext {
            store: store.clone(),
            identity: actor.clone(),
            ceiling: AuthorityCeiling::durable_execution(),
            read_deadline: None,
            installation_id: "local-installation".into(),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
        };
        let before = persisted(&store).await;
        assert_eq!(
            dispatch(context.clone(), action, params.clone())
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
        context.ceiling = AuthorityCeiling::trusted_local();
        let request = administration_authority_request(&context, action, &params)
            .await
            .unwrap();
        // The request precedes this change. The mutation must resolve current authority.
        store
            .set_team_member_role(
                AddTeamMemberInput::new(owner.clone(), "ordinary", "actor", TeamRole::Member)
                    .unwrap(),
            )
            .await
            .unwrap();
        let before = persisted(&store).await;
        let error = if action.ends_with("activate") {
            store
                .activate_team_member(
                    AddTeamMemberInput::new(actor.clone(), "ordinary", "target", TeamRole::Member)
                        .unwrap(),
                    request,
                )
                .await
                .unwrap_err()
        } else {
            store
                .unassign_team_project(
                    crate::access::ManageTeamProjectInput::new(
                        actor.clone(),
                        "ordinary",
                        "bootstrap-default",
                        None,
                    )
                    .unwrap(),
                    request,
                )
                .await
                .unwrap_err()
        };
        assert_eq!(map_access_error(error).kind(), "forbidden");
        assert_eq!(persisted(&store).await, before);
        store
            .set_team_member_role(
                AddTeamMemberInput::new(owner.clone(), "ordinary", "actor", TeamRole::Admin)
                    .unwrap(),
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn lifecycle_mutations_never_cross_organization_or_issuer_boundaries() {
    let (_directory, store, owner) = prepare().await;
    seed(&store, "target", "member").await;
    store
        .suspend_team_member(TeamMembershipInput::new(owner.clone(), "ordinary", "target").unwrap())
        .await
        .unwrap();
    call(
        &store,
        &owner,
        "access.team_project.assign",
        json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"member"}),
    )
    .await
    .unwrap();
    statement(&store,"INSERT INTO organizations VALUES('foreign','Foreign','active',1,1,1); INSERT INTO principals VALUES('foreign','foreign','user','active',NULL,1,1); INSERT INTO groups VALUES('foreign-team','foreign','team','Foreign','active',1,1,'foreign',1,1,NULL); INSERT INTO team_memberships VALUES('foreign-owner','foreign','foreign-team','foreign','owner','active',1,'foreign',1,1,NULL); INSERT INTO projects VALUES('foreign-project','foreign','Foreign','active',1,1,1)").await;
    for (action, params) in [
        (
            "access.team.member.activate",
            activation("foreign-team", "foreign", "member"),
        ),
        (
            "access.team.member.activate",
            activation("ordinary", "foreign", "member"),
        ),
        (
            "access.team_project.unassign",
            json!({"team_id":"foreign-team","project_id":"foreign-project"}),
        ),
        (
            "access.team_project.unassign",
            json!({"team_id":"ordinary","project_id":"foreign-project"}),
        ),
    ] {
        let before = persisted(&store).await;
        assert_eq!(
            call(&store, &owner, action, params)
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
    }
    statement(&store, "UPDATE principal_links SET issuer='https://different-issuer.invalid' WHERE principal_id='bootstrap-owner'").await;
    for (action, params) in [
        (
            "access.team.member.activate",
            activation("ordinary", "target", "member"),
        ),
        ("access.team_project.unassign", assignment()),
    ] {
        let before = persisted(&store).await;
        assert_eq!(
            call(&store, &owner, action, params)
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
        assert_eq!(persisted(&store).await, before);
    }
}

#[tokio::test]
async fn lifecycle_concurrent_replays_commit_only_one_transition() {
    let (directory, store, owner) = prepare().await;
    seed(&store, "target", "member").await;
    store
        .suspend_team_member(TeamMembershipInput::new(owner.clone(), "ordinary", "target").unwrap())
        .await
        .unwrap();
    call(
        &store,
        &owner,
        "access.team_project.assign",
        json!({"team_id":"ordinary","project_id":"bootstrap-default","role":"admin"}),
    )
    .await
    .unwrap();
    let other = AccessStore::open(directory.path().join("access.db"))
        .await
        .unwrap();
    for (action, params) in [
        (
            "access.team.member.activate",
            activation("ordinary", "target", "member"),
        ),
        ("access.team_project.unassign", assignment()),
    ] {
        let before = epochs(&store).await;
        let (first, second) = tokio::join!(
            call(&store, &owner, action, params.clone()),
            call(&other, &owner, action, params)
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let failure = first.err().or_else(|| second.err()).unwrap();
        assert_eq!(failure.kind(), "forbidden");
        let after = epochs(&store).await;
        assert_eq!(after[0], before[0] + 1);
        assert_eq!(after[1], before[1] + 1);
        assert_eq!(after[2], before[2] + 1);
    }
}
