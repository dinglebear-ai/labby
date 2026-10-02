use super::*;
fn entry(bytes: usize) -> Result<Metadata, ToolError> {
    Ok((
        Some("x".repeat(bytes)),
        vec![],
        BTreeMap::new(),
        None,
        "digest".into(),
    ))
}
#[test]
fn entries_bytes_errors_and_revision_replacement_are_bounded() {
    let mut cache = Cache::default();
    for n in 0..200 {
        cache.insert(format!("name-{n}"), "digest".into(), false, entry(100_000));
    }
    assert!(cache.bytes <= MAX_BYTES);
    assert!(cache.entries.len() <= MAX_ENTRIES);
    let count = cache.entries.len();
    let name = cache.entries.back().unwrap().0.clone();
    cache.insert(name.clone(), "changed".into(), false, entry(100_000));
    assert_eq!(cache.entries.len(), count);
    assert!(cache.bytes <= MAX_BYTES);
    assert_eq!(
        cache.entries.iter().filter(|entry| entry.0 == name).count(),
        1
    );
    assert_eq!(cache.entries.back().unwrap().1, "changed");
    cache.insert(
        "huge".into(),
        "digest".into(),
        false,
        entry(MAX_ENTRY_BYTES),
    );
    assert_eq!(cache.entries.len(), count);
    assert_eq!(cache.entries.back().unwrap().0, name);
    cache.insert(
        name.clone(),
        "oversized-revision".into(),
        false,
        entry(MAX_ENTRY_BYTES),
    );
    assert!(cache.entries.iter().all(|entry| entry.0 != name));
    assert_eq!(cache.entries.len(), count - 1);
    cache.insert(
        "error".into(),
        "digest".into(),
        false,
        Err(ToolError::InvalidParam {
            message: "x".repeat(100_000),
            param: "body".into(),
        }),
    );
    assert!(cache.bytes <= MAX_BYTES);
    assert!(cache.entries.back().unwrap().4 >= 100_000);
}
#[test]
fn nested_defaults_and_tool_strings_are_charged() {
    let mut inputs = BTreeMap::new();
    inputs.insert(
        "nested".into(),
        SnippetInputSpec {
            ty: SnippetInputType::Json,
            required: false,
            nullable: false,
            default: Some(serde_json::json!({"array":["x".repeat(10_000)]})),
            description: Some("description".into()),
        },
    );
    let tools =
        SnippetToolDeclarations::try_from(vec![format!("test::{}", "x".repeat(1000))]).unwrap();
    let metadata = Ok((
        None,
        vec!["tag".into()],
        inputs,
        Some(tools),
        "digest".into(),
    ));
    assert!(metadata_bytes(&metadata) > 11_000);
}
