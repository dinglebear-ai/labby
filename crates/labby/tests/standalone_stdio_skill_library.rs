#![cfg(feature = "skills")]

use std::process::Stdio;
use std::time::Duration;

use rmcp::{ServiceExt, model::GetSkillRequestParams, transport::TokioChildProcess};
use tokio::io::AsyncReadExt;

use labby_runtime::artifacts::{
    ArtifactProvenance, ArtifactStore, LibraryActorId, LibraryAuthorization, LibraryGrant,
    LibraryIdempotency, LibraryMutation, LibraryOwnership, LibraryTenantId, LibraryTimestamp,
    LogicalSkillFile, SkillLibraryRecord, SkillVisibility, canonical_json,
    materialize_logical_skill,
};

fn persist_active_skill(home: &std::path::Path) {
    let store = ArtifactStore::new(home.join("artifacts")).expect("artifact store");
    let ownership = LibraryOwnership::canonical(
        LibraryTenantId::from_canonical_projection("bootstrap-local").unwrap(),
        LibraryActorId::from_canonical_projection("bootstrap-owner").unwrap(),
    );
    let authorization = LibraryAuthorization::from_authorized_access_projection(
        ownership.tenant_id.clone(),
        ownership.owner_id.clone(),
        LibraryGrant::Owner,
    );
    let materialized = materialize_logical_skill(
        "persisted-stdio",
        vec![LogicalSkillFile::new(
            "SKILL.md",
            "---\nname: persisted-stdio\ndescription: Loaded during standalone stdio bootstrap\n---\n\nPersisted body.\n",
        )],
        ArtifactProvenance::default(),
    )
    .expect("materialized skill");
    let artifact_id = materialized.interchange.descriptor.id.clone();
    let revision_id = materialized.interchange.revision.id.clone();
    let now = LibraryTimestamp::parse("2026-08-26T00:00:00Z").unwrap();
    store
        .mutate_library_with_materialized_outcome(
            &authorization,
            &ownership,
            0,
            LibraryIdempotency {
                key: "persist-stdio-skill".to_owned(),
                request_digest: canonical_json::digest(&"persist-stdio-skill").unwrap(),
                terminal_audit: None,
            },
            LibraryMutation::Create {
                record: SkillLibraryRecord {
                    artifact_id: artifact_id.clone(),
                    name: "persisted-stdio".to_owned(),
                    ownership: ownership.clone(),
                    visibility: SkillVisibility::Tenant,
                    archived: false,
                    active_revision_id: None,
                    latest_revision_id: revision_id.clone(),
                    latest_revision_files: Vec::new(),
                    search_metadata: Vec::new(),
                    provenance_provider: None,
                    materialized: false,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            },
            now.clone(),
            materialized,
            None,
            |_| Ok(()),
        )
        .expect("persist skill");
    store
        .mutate_library(
            &authorization,
            &ownership,
            1,
            LibraryIdempotency {
                key: "activate-stdio-skill".to_owned(),
                request_digest: canonical_json::digest(&"activate-stdio-skill").unwrap(),
                terminal_audit: None,
            },
            LibraryMutation::Activate {
                artifact_id: artifact_id.clone(),
                revision_id,
                updated_at: now.clone(),
            },
            now,
        )
        .expect("activate persisted skill");
}

#[tokio::test]
async fn standalone_stdio_bootstraps_persisted_active_skills() {
    let home = tempfile::tempdir().expect("LABBY_HOME");
    persist_active_skill(home.path());
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .args(["mcp", "--services", "artifacts"])
        .env("LABBY_HOME", home.path())
        .env("LABBY_SERVER_URL", "")
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_MCP_HTTP_PORT", "0")
        .env("LABBY_LOG", "labby=debug,warn")
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(30), async {
        let (transport, stderr) = TokioChildProcess::builder(command)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let logs = tokio::spawn(async move {
            let mut logs = String::new();
            stderr.unwrap().read_to_string(&mut logs).await.unwrap();
            logs
        });
        let client = ().serve(transport).await.expect("initialize stdio client");
        let listed = client.list_skills(None).await.expect("list served Skills");
        assert!(
            listed
                .skills
                .iter()
                .any(|s| s.uri.as_str() == "skill://labby/using-labby/SKILL.md")
        );
        // A standalone stdio request has no authenticated Artifact project.
        assert!(
            !listed
                .skills
                .iter()
                .any(|s| s.uri.as_str() == "skill://labby/persisted-stdio/SKILL.md")
        );
        client
            .get_skill(GetSkillRequestParams::new(
                "skill://labby/persisted-stdio/SKILL.md",
            ))
            .await
            .expect_err("protected artifact skill must require project authority");
        client.cancel().await.expect("close stdio client");
        let logs = logs.await.unwrap();
        assert!(logs.contains("artifacts.ready") && logs.contains("active_skill_count=1"));
        assert!(
            logs.lines()
                .any(|line| line.contains("captured Skill generation")
                    && line.contains("skill_generation=2")),
            "the native Skills request did not capture the persisted generation"
        );
    })
    .await
    .expect("standalone stdio test timed out");
}
