//! Memory bounds for the `skills/list` ownership pre-scan in `ingest_page`.

use super::*;
use labby_runtime::skills::wire::SkillResource;

fn entry(uri: &str, resources: Vec<SkillResource>) -> SkillEntry {
    serde_json::from_value(serde_json::json!({
        "uri": uri,
        "frontmatter": {"name": "x", "description": "x"},
        "resources": resources
            .iter()
            .map(|resource| serde_json::json!({
                "uri": resource.uri,
                "digest": resource.digest,
                "size": resource.size,
            }))
            .collect::<Vec<_>>(),
    }))
    .expect("skill entry")
}

fn resource(uri: String) -> SkillResource {
    SkillResource {
        uri,
        digest: String::new(),
        size: 0,
    }
}

#[test]
fn oversized_entry_uri_never_enters_the_ownership_scan() {
    // An over-long owner URI is rejected by validation, so its many claims
    // must never reach the scratch map.
    let owner = format!(
        "skill://hostile/{}/SKILL.md",
        "a".repeat(limits::MAX_URI_CHARS)
    );
    let claims = (0..2_000)
        .map(|index| resource(format!("skill://hostile/claim-{index}")))
        .collect();
    let mut out = UpstreamSkills::default();
    let mut owners = ObservedResourceOwners::new();
    ingest_page(
        vec![Ok(entry(&owner, claims))],
        &mut out,
        &mut owners,
        limits::MAX_SKILLS_PER_UPSTREAM,
        limits::MAX_SKILL_CANDIDATES_PER_UPSTREAM,
    );
    assert!(owners.is_empty());
    assert_eq!(out.skills.len(), 0);
    assert_eq!(
        out.excluded.len(),
        1,
        "the entry is still excluded by validation"
    );
}

#[test]
fn over_long_resource_manifest_never_enters_the_ownership_scan() {
    let claims = (0..=limits::MAX_RESOURCES_PER_SKILL)
        .map(|index| resource(format!("skill://wide/file-{index}")))
        .collect();
    let mut out = UpstreamSkills::default();
    let mut owners = ObservedResourceOwners::new();
    ingest_page(
        vec![Ok(entry("skill://wide/SKILL.md", claims))],
        &mut out,
        &mut owners,
        limits::MAX_SKILLS_PER_UPSTREAM,
        limits::MAX_SKILL_CANDIDATES_PER_UPSTREAM,
    );
    assert!(owners.is_empty());
    assert_eq!(out.excluded.len(), 1);
}

#[test]
fn in_limit_entries_share_one_owner_allocation_across_claims() {
    let claims = (0..8)
        .map(|index| resource(format!("skill://shared/file-{index}")))
        .collect();
    let mut out = UpstreamSkills::default();
    let mut owners = ObservedResourceOwners::new();
    ingest_page(
        vec![Ok(entry("skill://shared/SKILL.md", claims))],
        &mut out,
        &mut owners,
        limits::MAX_SKILLS_PER_UPSTREAM,
        limits::MAX_SKILL_CANDIDATES_PER_UPSTREAM,
    );
    let shared: Vec<_> = owners.values().map(|(owner, _, _)| owner).collect();
    assert!(shared.len() >= 8);
    assert!(
        shared.windows(2).all(|pair| Arc::ptr_eq(pair[0], pair[1])),
        "claims must share the owner, not clone it"
    );
}

#[test]
fn conflicting_claims_across_owners_are_still_ambiguous() {
    let shared = || vec![resource("skill://a/shared.md".to_string())];
    let mut first = shared();
    first[0].digest = "sha256:one".to_string();
    let mut second = shared();
    second[0].digest = "sha256:two".to_string();
    let mut out = UpstreamSkills::default();
    let mut owners = ObservedResourceOwners::new();
    ingest_page(
        vec![
            Ok(entry("skill://a/SKILL.md", first)),
            Ok(entry("skill://a/b/SKILL.md", second)),
        ],
        &mut out,
        &mut owners,
        limits::MAX_SKILLS_PER_UPSTREAM,
        limits::MAX_SKILL_CANDIDATES_PER_UPSTREAM,
    );
    assert!(out.ambiguous_resource_uris.contains("skill://a/shared.md"));
}

#[test]
fn every_entry_validation_accepts_takes_part_in_the_ownership_scan() {
    // The pre-scan must never skip an entry that validation would accept, or a
    // valid owner's claims could escape ambiguity detection.
    use labby_runtime::skills::ResourceDigest;
    let body = "---\nname: brief\ndescription: a test skill\n---\n\n# Body\n";
    let notes = b"supporting notes";
    let cases = [
        ("skill://valid/brief/SKILL.md", 0usize),
        ("skill://valid/brief/SKILL.md", 1),
        (
            "skill://valid/brief/SKILL.md",
            limits::MAX_RESOURCES_PER_SKILL - 1,
        ),
    ];
    for (uri, supporting) in cases {
        let mut resources = vec![SkillResource {
            uri: uri.to_string(),
            digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
            size: body.len() as u64,
        }];
        for index in 0..supporting {
            resources.push(SkillResource {
                uri: format!("skill://valid/brief/notes-{index}.md"),
                digest: ResourceDigest::of_bytes(notes).to_wire(),
                size: notes.len() as u64,
            });
        }
        let entry = SkillEntry {
            uri: uri.to_string(),
            frontmatter: serde_json::from_value(
                serde_json::json!({"name": "brief", "description": "a test skill"}),
            )
            .expect("frontmatter"),
            resources: Some(resources),
            meta: None,
        };
        let rejection = validate_skill_entry_detailed(&entry)
            .err()
            .map(|rejection| rejection.detail);
        assert!(
            rejection.is_none(),
            "fixture must be valid ({supporting} supporting files): {rejection:?}"
        );
        assert!(within_ownership_scan_limits(&entry));
    }
}
