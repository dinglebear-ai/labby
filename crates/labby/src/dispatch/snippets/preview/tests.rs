use super::*;

fn descriptor_preparation(entry: &CatalogDescriptor) -> PreparedSnippet {
    let scope = ToolScope::new(vec!["alpha".into()], vec![entry.id.clone()]);
    let input = json!({"topic":"unchanged"});
    let config = labby_runtime::CodeModeConfig::default();
    let mut budget = MAX_DESCRIPTOR_BYTES;
    let schema_digests =
        BTreeMap::from([(entry.id.clone(), descriptor_digest(entry, &mut budget))]);
    let fingerprints = PreviewFingerprints {
        snippet_digest: "same-source".into(),
        input_digest: value_digest(&input),
        effective_scope_fingerprint: scope.fingerprint(),
        runtime_version: "same-runtime".into(),
        tool_schema_digest: value_digest(&json!({"descriptors":schema_digests,"catalog":null})),
    };
    let preview_fingerprint = fingerprint::assemble(
        "demo",
        &fingerprints,
        &config,
        &SnippetDispatchContext::trusted_local(),
        None,
        &[],
    );
    let body = "async () => ({ ok: true })".to_owned();
    PreparedSnippet {
        snippet: ResolvedSnippet {
            content_digest: None,
            tools: None,
            name: "demo".into(),
            description: None,
            tags: vec![],
            inputs: BTreeMap::new(),
            source: super::super::store::SnippetSource::User,
            path: "demo.md".into(),
            body: body.clone(),
        },
        code: body,
        input,
        scope,
        config,
        schema_digests: Some(schema_digests),
        preview: SnippetPreview {
            name: "demo".into(),
            execution_id: None,
            mode: "metadata".into(),
            dynamic_unknown: true,
            coverage: "declared_tools_only".into(),
            can_execute: true,
            input_summary: PreviewInputSummary {
                keys: vec!["topic".into()],
                provided_keys: vec!["topic".into()],
                defaulted_keys: vec![],
            },
            declared_tools: vec![],
            fingerprints,
            preview_fingerprint,
            drift: vec![],
            warnings: vec![],
        },
    }
}

#[test]
fn schema_and_safety_only_changes_reject_the_previous_normal_execution_guard() {
    let original = CatalogDescriptor::tool(
        "alpha",
        "read",
        "same description",
        Some(json!({"type":"object"})),
        Some(json!({"type":"object"})),
    );
    let before = descriptor_preparation(&original);
    let expected = &before.preview.preview_fingerprint;
    check_guard(&before, expected).expect("unchanged preparation accepts its own preview");
    let mut input_change = original.clone();
    input_change.schema = Some(json!({"type":"object","required":["topic"]}));
    let mut output_change = original.clone();
    output_change.output_schema = Some(json!({"type":"array"}));
    let mut safety_change = original.clone();
    safety_change.safety = Some(CodeModeToolSafety {
        read_only: Some(true),
        destructive: Some(false),
    });
    for changed in [input_change, output_change, safety_change] {
        let now = descriptor_preparation(&changed);
        assert_eq!(
            before.preview.fingerprints.snippet_digest,
            now.preview.fingerprints.snippet_digest
        );
        assert_eq!(
            before.preview.fingerprints.input_digest,
            now.preview.fingerprints.input_digest
        );
        assert_eq!(
            before.preview.fingerprints.effective_scope_fingerprint,
            now.preview.fingerprints.effective_scope_fingerprint
        );
        assert_eq!(
            before.preview.fingerprints.runtime_version,
            now.preview.fingerprints.runtime_version
        );
        assert_eq!(
            check_guard(&now, expected).unwrap_err().kind(),
            "preview_stale"
        );
    }
}

#[test]
fn read_only_preview_never_treats_unknown_or_mutating_descriptors_as_allowed() {
    let mut entry = CatalogDescriptor::tool("alpha", "read", "read", None, None);
    let scope = ToolScope::default().read_only();
    assert!(!descriptor_access_allowed(&scope, &entry));
    entry.safety = Some(CodeModeToolSafety {
        read_only: Some(false),
        destructive: None,
    });
    assert!(!descriptor_access_allowed(&scope, &entry));
    entry.safety = Some(CodeModeToolSafety {
        read_only: Some(true),
        destructive: Some(false),
    });
    assert!(descriptor_access_allowed(&scope, &entry));
    assert!(!descriptor_access_allowed(
        &ToolScope::scoped_namespaces(vec!["other".into()], vec![]),
        &entry
    ));
}

fn receipt() -> SnippetExecutionReceipt {
    SnippetExecutionReceipt {
        execution_id: "old".into(),
        snippet_name: "demo".into(),
        snippet_digest: "old-source".into(),
        input_digest: "old-input".into(),
        effective_scope_fingerprint: "old-scope".into(),
        runtime_version: "old-runtime".into(),
        tool_schema_digests: Some(BTreeMap::from([(
            "alpha::read".into(),
            Some("old-schema".into()),
        )])),
        surface: "api".into(),
        created_at_ms: 0,
        elapsed_ms: 0,
        status: "started".into(),
        error_kind: None,
        result_digest: None,
        result_bytes: None,
        calls: vec![],
        tool_calls: 0,
        omitted_calls: 0,
        artifacts: vec![],
    }
}

#[test]
fn all_reproducibility_fields_are_compared_without_using_payloads_or_completion_status() {
    let old = receipt();
    let now = PreviewFingerprints {
        snippet_digest: "new-source".into(),
        input_digest: "new-input".into(),
        effective_scope_fingerprint: "new-scope".into(),
        runtime_version: "new-runtime".into(),
        tool_schema_digest: "new-schema".into(),
    };
    let schemas = BTreeMap::from([("alpha::read".into(), Some("new-schema".into()))]);
    let differences = drift(&old, &now, Some(&schemas));
    assert_eq!(
        differences
            .iter()
            .map(|item| item.field.as_str())
            .collect::<Vec<_>>(),
        [
            "snippet",
            "input",
            "effective_scope",
            "runtime",
            "tool_schema"
        ]
    );
    assert!(differences.iter().all(|item| item.status == "changed"));
    let mut unknown = old.clone();
    unknown.tool_schema_digests = None;
    assert_eq!(
        drift(&unknown, &now, Some(&schemas)).last().unwrap().status,
        "unverifiable"
    );
    let partial = BTreeMap::from([("alpha::read".into(), None)]);
    assert_eq!(
        drift(&old, &now, Some(&partial)).last().unwrap().status,
        "unverifiable"
    );
}

#[test]
fn descriptor_evidence_includes_safety_and_both_schemas_and_stops_at_byte_budget() {
    fn fingerprint(entry: &CatalogDescriptor) -> String {
        let mut budget = MAX_DESCRIPTOR_BYTES;
        descriptor_digest(entry, &mut budget).unwrap()
    }
    let mut entry = CatalogDescriptor::tool(
        "alpha",
        "read",
        "read",
        Some(json!({"type":"object"})),
        Some(json!({"type":"string"})),
    );
    let initial = fingerprint(&entry);
    entry.safety = Some(CodeModeToolSafety {
        read_only: Some(true),
        destructive: Some(false),
    });
    let safety = fingerprint(&entry);
    assert_ne!(initial, safety);
    entry.output_schema = Some(json!({"type":"number"}));
    assert_ne!(safety, fingerprint(&entry));
    entry.schema = Some(json!({"description":"x".repeat(MAX_DESCRIPTOR_BYTES+1)}));
    let mut remaining = MAX_DESCRIPTOR_BYTES;
    assert!(descriptor_digest(&entry, &mut remaining).is_none());
    assert_eq!(
        remaining, MAX_DESCRIPTOR_BYTES,
        "oversized evidence cannot consume another descriptor's allowance"
    );
}
