use super::*;
use serde_json::json;

fn body(nullable: bool) -> String {
    format!(
        "---\nname: demo\ndescription: Demo\ninputs:\n  value:\n    type: string\n    nullable: {nullable}\n---\n```js\nasync (input) => input\n```\n"
    )
}

#[test]
fn explicit_null_requires_nullable_but_omission_remains_optional() {
    let home = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    create_user_snippet(home.path(), "demo", &body(false), None, false).unwrap();
    let snippet = resolve_snippet(home.path(), builtin.path(), "demo").unwrap();
    assert!(merge_snippet_input(&snippet, json!({"value":null})).is_err());
    assert_eq!(
        merge_snippet_input(&snippet, json!({})).unwrap(),
        json!({"value":null})
    );
    create_user_snippet(home.path(), "demo", &body(true), None, true).unwrap();
    let snippet = resolve_snippet(home.path(), builtin.path(), "demo").unwrap();
    assert!(merge_snippet_input(&snippet, json!({"value":null})).is_ok());
}

#[test]
fn stale_digest_does_not_replace_newer_source() {
    let home = tempfile::tempdir().unwrap();
    let first = create_user_snippet(home.path(), "demo", &body(false), None, false).unwrap();
    create_user_snippet(home.path(), "demo", &body(true), None, true).unwrap();
    assert!(
        create_user_snippet_checked(
            home.path(),
            "demo",
            &body(false),
            None,
            true,
            first.content_digest.as_deref()
        )
        .is_err()
    );
    assert!(
        read_snippet_body(&first.path)
            .unwrap()
            .contains("nullable: true")
    );
}

#[test]
fn invalid_override_reports_failure_and_shadows_builtin() {
    let home = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    fs::write(builtin.path().join("demo.md"), body(false)).unwrap();
    fs::create_dir_all(user_snippet_dir(home.path())).unwrap();
    fs::write(
        user_snippet_dir(home.path()).join("demo.md"),
        "invalid source",
    )
    .unwrap();
    let listed = list_snippets_with_diagnostics(home.path(), builtin.path()).unwrap();
    assert_eq!(listed.diagnostics.len(), 1);
    assert_eq!(listed.snippets.len(), 1);
    assert!(listed.snippets[0].shadowed);
    let resolved = resolve_snippet(home.path(), builtin.path(), "demo").unwrap();
    assert_eq!(resolved.source, SnippetSource::User);
    assert!(code_for_snippet(&resolved).is_err());
}

#[test]
fn cache_refreshes_by_content_not_file_metadata() {
    let home = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let info = create_user_snippet(home.path(), "demo", &body(false), None, false).unwrap();
    let first = list_snippets(home.path(), builtin.path()).unwrap();
    fs::write(&info.path, body(true)).unwrap();
    let second = list_snippets(home.path(), builtin.path()).unwrap();
    assert_ne!(first[0].content_digest, second[0].content_digest);
    assert!(second[0].inputs["value"].nullable);
}

#[test]
fn required_null_is_rejected_and_nullable_null_default_is_supported() {
    let home = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let source = body(false).replace("    type: string", "    type: string\n    required: true");
    create_user_snippet(home.path(), "demo", &source, None, false).unwrap();
    let snippet = resolve_snippet(home.path(), builtin.path(), "demo").unwrap();
    assert!(merge_snippet_input(&snippet, json!({"value":null})).is_err());
    assert!(merge_snippet_input(&snippet, json!({})).is_err());
    let source = body(true).replace("    type: string", "    type: string\n    default: null");
    create_user_snippet(home.path(), "demo", &source, None, true).unwrap();
    let snippet = resolve_snippet(home.path(), builtin.path(), "demo").unwrap();
    assert_eq!(
        merge_snippet_input(&snippet, json!({})).unwrap(),
        json!({"value":null})
    );
}

#[test]
fn editing_legacy_js_keeps_one_override_and_metadata() {
    let home = tempfile::tempdir().unwrap();
    let dir = user_snippet_dir(home.path());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("demo.js"), b"async () => ({ok:true})").unwrap();
    let info = create_user_snippet(home.path(), "demo", &body(true), None, true).unwrap();
    assert_eq!(info.path, dir.join("demo.js"));
    assert!(!dir.join("demo.md").exists());
    let snippet = resolve_snippet(home.path(), &dir, "demo").unwrap();
    assert!(snippet.inputs["value"].nullable);
}

#[test]
fn subprocess_create_worker() {
    let Ok(dir) = std::env::var("LABBY_SNIPPET_CREATE_TEST_HOME") else {
        return;
    };
    let result = create_user_snippet(Path::new(&dir), "demo", &body(false), None, false);
    // Distinct exit codes let the parent assert exactly one winner.
    std::process::exit(if result.is_ok() { 0 } else { 3 });
}

#[test]
fn cross_process_create_has_exactly_one_winner() {
    let home = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap();
    let spawn = || {
        std::process::Command::new(&executable)
            .args([
                "--exact",
                "snippet::store::reliability_tests::subprocess_create_worker",
            ])
            .env("LABBY_SNIPPET_CREATE_TEST_HOME", home.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let mut first = spawn();
    let mut second = spawn();
    let mut codes = [
        first.wait().unwrap().code().unwrap(),
        second.wait().unwrap().code().unwrap(),
    ];
    codes.sort_unstable();
    assert_eq!(codes, [0, 3]);
    assert_eq!(
        list_snippets(home.path(), &home.path().join("builtin"))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn cached_invalid_override_recovers_when_contents_change() {
    let home = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let dir = user_snippet_dir(home.path());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("demo.md"), "async () => broken(").unwrap();
    assert_eq!(
        list_snippets_with_diagnostics(home.path(), builtin.path())
            .unwrap()
            .diagnostics
            .len(),
        1
    );
    // The invalid content result is cached too; changing bytes invalidates it.
    fs::write(dir.join("demo.md"), body(false)).unwrap();
    let listed = list_snippets_with_diagnostics(home.path(), builtin.path()).unwrap();
    assert!(listed.diagnostics.is_empty());
    assert_eq!(listed.snippets.len(), 1);
}

#[test]
fn builder_json_defaults_preserve_string_types_and_escapes() {
    let home = tempfile::tempdir().unwrap();
    let body = "---\nname: builder\ndescription: \"Quoted \\\"description\\\"\"\ninputs:\n  numeric_text:\n    type: string\n    default: \"123\"\n  bool_text:\n    type: string\n    default: \"true\"\n  escaped:\n    type: string\n    default: \"a\\n\\\"b\"\n---\n```js\nasync (input) => input\n```\n";
    create_user_snippet(home.path(), "builder", body, None, false).unwrap();
    let snippet = resolve_snippet(home.path(), home.path(), "builder").unwrap();
    let params = merge_snippet_input(&snippet, json!({})).unwrap();
    assert_eq!(params["numeric_text"], "123");
    assert_eq!(params["bool_text"], "true");
    assert_eq!(params["escaped"], "a\n\"b");
    assert_eq!(
        snippet.description.as_deref(),
        Some("Quoted \"description\"")
    );
}

#[test]
fn discovery_diagnostics_never_expose_invalid_source_and_are_bounded() {
    let home = tempfile::tempdir().unwrap();
    let dir = user_snippet_dir(home.path());
    fs::create_dir_all(&dir).unwrap();
    for i in 0..130 {
        fs::write(dir.join(format!("invalid-{i}.md")), "---\nname: invalid\ndescription: test\nsensitive-invalid-line-sk-placeholder\n---\n```js\nasync () => true\n```\n").unwrap();
    }
    let listed = list_snippets_with_diagnostics(home.path(), home.path()).unwrap();
    assert_eq!(listed.diagnostics.len(), 128);
    assert_eq!(listed.diagnostics_omitted, 2);
    assert!(
        listed
            .diagnostics
            .iter()
            .all(|d| !d.message.contains("sk-placeholder"))
    );
}

#[test]
fn generated_fanout_draft_uses_supported_metadata_and_mapped_inputs() {
    let home = tempfile::tempdir().unwrap();
    let body = r#"---
name: fanout-check
description: Reusable Labby workflow
tags: [builder, fanout]
inputs:
  query:
    type: string
    default: "hello"
    required: false
tools:
  - "first::search"
  - "second::status"
---
```js
async (input) => {
  const results = await codemode.batch([
    () => callTool("first::search", {"query": input["query"]}),
    () => callTool("second::status", {"detail": true})
  ]);
  return results;
}
```
"#;
    let info = create_user_snippet(home.path(), "fanout-check", body, None, false).unwrap();
    assert_eq!(info.inputs["query"].default, Some(json!("hello")));
    assert_eq!(info.tools.unwrap().as_slice().len(), 2);
}

#[test]
fn listing_bounds_diagnostics_and_redacts_invalid_source_values() {
    let home = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let dir = user_snippet_dir(home.path());
    fs::create_dir_all(&dir).unwrap();
    for index in 0..130 {
        let source = format!(
            "---\nname: invalid-{index}\ndescription: Test\ninputs:\n  token:\n    type: PRIVATE_SOURCE_TOKEN\n---\n```js\nasync () => null\n```\n"
        );
        fs::write(dir.join(format!("invalid-{index}.md")), source).unwrap();
    }
    let list = list_snippets_with_diagnostics(home.path(), builtin.path()).unwrap();
    assert_eq!(list.diagnostics.len(), 128);
    assert_eq!(list.diagnostics_omitted, 2);
    assert!(
        !serde_json::to_string(&list)
            .unwrap()
            .contains("PRIVATE_SOURCE_TOKEN")
    );
}

#[test]
fn quoted_json_defaults_roundtrip_without_losing_string_types_or_escapes() {
    let defaults = [
        ("\"true\"", json!("true")),
        ("\"null\"", json!("null")),
        ("\"line\\nquote\\\"\"", json!("line\nquote\"")),
    ];
    for (literal, expected) in defaults {
        let source = body(false).replace(
            "    type: string",
            &format!("    type: string\n    default: {literal}"),
        );
        let parsed = frontmatter(&source).unwrap().unwrap();
        assert_eq!(parsed.inputs["value"].default, Some(expected));
    }
}

#[test]
fn duplicate_extensions_list_only_the_source_resolver_will_execute() {
    let home = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let dir = user_snippet_dir(home.path());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("demo.md"), body(false)).unwrap();
    fs::write(dir.join("demo.js"), b"async () => ({alternate:true})").unwrap();
    let list = list_snippets_with_diagnostics(home.path(), builtin.path()).unwrap();
    assert_eq!(list.snippets.len(), 1);
    assert_eq!(list.snippets[0].path, dir.join("demo.md"));
    assert_eq!(list.diagnostics.len(), 1);
    assert_eq!(list.diagnostics[0].path, dir.join("demo.js"));
    assert_eq!(
        resolve_snippet(home.path(), builtin.path(), "demo")
            .unwrap()
            .path,
        list.snippets[0].path
    );
    // Invalid preferred metadata cannot silently activate the alternate extension.
    fs::write(dir.join("demo.md"), "async () => invalid(").unwrap();
    let list = list_snippets_with_diagnostics(home.path(), builtin.path()).unwrap();
    assert!(list.snippets.is_empty());
    assert_eq!(list.diagnostics.len(), 2);
}
