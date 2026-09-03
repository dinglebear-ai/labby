#![cfg(feature = "skills")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

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

fn read_response(reader: &mut BufReader<std::process::ChildStdout>, id: u64) -> serde_json::Value {
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read stdio response");
        assert!(!line.is_empty(), "stdio server exited before response {id}");
        let value: serde_json::Value = serde_json::from_str(line.trim()).expect("JSON-RPC line");
        if value["id"] == id {
            return value;
        }
    }
}

#[test]
fn standalone_stdio_bootstraps_persisted_active_skills() {
    let home = tempfile::tempdir().expect("LABBY_HOME");
    persist_active_skill(home.path());

    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["mcp", "--services", "artifacts"])
        .env("LABBY_HOME", home.path())
        .env("LABBY_SERVER_URL", "")
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_LOG", "labby=info,warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn standalone stdio server");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "stdio-bootstrap-test", "version": "1"}
            }
        })
    )
    .unwrap();
    assert!(read_response(&mut stdout, 1).get("result").is_some());
    drop(stdin);
    let output = child.wait_with_output().expect("wait for stdio server");
    assert!(
        output.status.success(),
        "standalone stdio exited unsuccessfully"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("artifacts.ready") && stderr.contains("active_skill_count=1"),
        "standalone stdio did not bootstrap the persisted active generation: {stderr}"
    );
}
