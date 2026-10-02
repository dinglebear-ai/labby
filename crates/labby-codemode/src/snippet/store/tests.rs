#![allow(clippy::panic)]
use super::*;
use serde_json::json;

fn valid_body() -> &'static str {
    "---\nname: demo\ndescription: Demo snippet\ntags: []\n---\n\n```js\nasync () => ({ ok: true })\n```\n"
}

#[test]
fn generated_frontmatter_description_is_single_line() {
    let body = render_user_snippet_body(
        "demo",
        "async () => ({ ok: true })",
        Some("first line\nname: injected\n---\nsecond line"),
    )
    .expect("rendered");

    let metadata = frontmatter(&body)
        .expect("frontmatter parsed")
        .expect("metadata");
    assert_eq!(metadata.name, "demo");
    assert_eq!(
        metadata.description,
        "first line name: injected --- second line"
    );
    assert!(validate_snippet_body("demo", &body).is_ok());
}

#[test]
fn frontmatter_requires_exact_closing_delimiter_line() {
    let body = "---\nname: demo\ndescription: Demo snippet\n--- trailing\n\n```js\nasync () => ({ ok: true })\n```\n";

    let error = frontmatter(body).expect_err("loose delimiter should be rejected");
    assert!(format!("{error}").contains("not closed"));
}

#[test]
fn validate_snippet_body_accepts_valid_frontmatter() {
    assert!(validate_snippet_body("demo", valid_body()).is_ok());
}

#[test]
fn validate_snippet_code_accepts_formatter_trailing_semicolon() {
    let code = "async () => ({ ok: true });";
    assert!(validate_snippet_code(code).is_ok());
    assert_eq!(normalize_snippet_code(code), "async () => ({ ok: true })");
}

#[test]
fn validate_snippet_code_accepts_trailing_line_comment() {
    let code = "async () => ({ ok: true }) // formatter note";
    assert!(
        validate_snippet_code(code).is_ok(),
        "the validator's generated closing delimiter must not be swallowed by a trailing // comment"
    );
}

#[test]
fn validate_snippet_body_rejects_malformed_javascript_before_execution() {
    let body = "---\nname: demo\ndescription: Broken snippet\ntags: []\n---\n\n```js\nasync () => { const broken = ; return broken; }\n```\n";
    let error = validate_snippet_body("demo", body)
        .expect_err("malformed JavaScript must fail static validation");
    assert!(
        format!("{error}").contains("snippet JavaScript is invalid"),
        "syntax failure should explain that the JavaScript is invalid: {error}"
    );
}

#[test]
fn validate_snippet_code_parses_without_executing_function_body() {
    let code = "async () => { throw new Error(\"validation must not execute me\"); }";
    assert!(
        validate_snippet_code(code).is_ok(),
        "validation should compile the function expression without invoking it"
    );
}

#[test]
fn resolve_snippet_not_found_does_not_expose_filesystem_authorities() {
    let lab_home = tempfile::tempdir().expect("lab home");
    let builtin = tempfile::tempdir().expect("builtin snippets");
    let error = resolve_snippet(lab_home.path(), builtin.path(), "missing")
        .expect_err("missing snippet must fail");
    let message = format!("{error}");
    assert!(message.contains("missing"));
    assert!(!message.contains(&user_snippet_dir(lab_home.path()).display().to_string()));
    assert!(!message.contains(&builtin.path().display().to_string()));
}

#[test]
fn atomic_write_snippet_rejects_overwrite_without_force_under_lock() {
    // Publication rechecks existence under the cross-process directory lock.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("demo.md");
    let body = render_user_snippet_body("demo", "async () => ({ ok: true })", None)
        .expect("rendered body");

    publication::publish(path.parent().unwrap(), "demo", &body, false, None)
        .expect("first write succeeds");

    let err = publication::publish(path.parent().unwrap(), "demo", &body, false, None)
        .expect_err("second non-force write must be rejected");
    assert!(
        matches!(err, ToolError::Conflict { .. }),
        "under-lock guard must reject overwrite without force, got {err:?}"
    );

    // force=true still overwrites in place.
    publication::publish(path.parent().unwrap(), "demo", &body, true, None)
        .expect("force write overwrites");
}

#[test]
fn frontmatter_tolerates_crlf_line_endings() {
    // Regression guard for the Windows-checkout failure: a `---\r\n` opening
    // delimiter must be recognized just like `---\n`, otherwise built-in
    // snippets carry no frontmatter and become undiscoverable. Without this
    // test a dropped CRLF arm in `strip_frontmatter_open` would pass on both
    // CI platforms (Cargo never rewrites these string literals).
    let body = "---\r\nname: demo\r\ndescription: Demo snippet\r\ntags: []\r\n---\r\n\r\n```js\r\nasync () => ({ ok: true })\r\n```\r\n";

    assert!(has_frontmatter(body), "CRLF frontmatter must be detected");
    let meta = frontmatter(body)
        .expect("CRLF frontmatter must parse")
        .expect("CRLF frontmatter must be present");
    assert_eq!(meta.name, "demo");
    assert_eq!(meta.description, "Demo snippet");
    assert!(validate_snippet_body("demo", body).is_ok());
}

#[test]
fn snippet_catalog_exposes_metadata_without_saved_source() {
    const SOURCE_SENTINEL: &str = "SOURCE_SENTINEL_MUST_STAY_EXECUTION_SIDE";
    let lab_home = tempfile::tempdir().expect("temp lab home");
    let builtin_dir = tempfile::tempdir().expect("temp builtin dir");
    let code = format!(
        "async () => {{ const marker = \"{SOURCE_SENTINEL}\"; return {{ ok: marker.length > 0 }}; }}"
    );
    create_user_snippet(
        lab_home.path(),
        "metadata-only",
        &code,
        Some("Metadata-only catalog oracle"),
        false,
    )
    .expect("create user snippet");

    let listed =
        list_snippets(lab_home.path(), builtin_dir.path()).expect("list saved snippet metadata");
    let serialized = serde_json::to_string(&listed).expect("serialize catalog metadata");
    assert!(serialized.contains("metadata-only"));
    assert!(serialized.contains("Metadata-only catalog oracle"));
    assert!(
        !serialized.contains(SOURCE_SENTINEL),
        "saved source must not enter model-facing snippet catalog metadata"
    );

    let resolved = resolve_snippet(lab_home.path(), builtin_dir.path(), "metadata-only")
        .expect("host-side source resolution");
    assert!(
        resolved.body.contains(SOURCE_SENTINEL),
        "source must remain available to the execution plane"
    );
}

#[test]
fn docker_host_inventory_uses_per_run_log_markers() {
    let lab_home = tempfile::tempdir().expect("temp lab home");
    let snippet = resolve_snippet(
        lab_home.path(),
        &builtin_snippet_dir(),
        "docker-host-inventory",
    )
    .expect("resolve docker host inventory");
    let code = code_for_snippet(&snippet).expect("valid docker inventory source");

    assert!(
        code.contains("Math.random()"),
        "log framing must carry a per-run nonce"
    );
    assert!(
        code.contains("__LABBY_DOCKER_LOG_SECTION_${markerNonce}__"),
        "section marker must incorporate the per-run nonce"
    );
    assert!(
        !code.contains("__LABBY_DOCKER_LOG_SECTION_9D81__"),
        "static framing lets container output spoof parser boundaries"
    );
}

#[test]
fn homelab_inventory_snippets_pin_ssh_safety_and_artifact_redaction() {
    let lab_home = tempfile::tempdir().expect("temp lab home");
    let builtin = builtin_snippet_dir();
    let ssh = code_for_snippet(
        &resolve_snippet(lab_home.path(), &builtin, "homelab-ssh-targets")
            .expect("resolve ssh targets"),
    )
    .expect("valid ssh targets source");
    let docker = code_for_snippet(
        &resolve_snippet(lab_home.path(), &builtin, "docker-host-inventory")
            .expect("resolve docker host inventory"),
    )
    .expect("valid docker host source");
    let aggregate = code_for_snippet(
        &resolve_snippet(lab_home.path(), &builtin, "homelab-docker-inventory")
            .expect("resolve aggregate inventory"),
    )
    .expect("valid aggregate source");

    for code in [&ssh, &docker] {
        assert!(code.contains("-o ForwardAgent=no"));
        assert!(code.contains("-o ClearAllForwardings=yes"));
        assert!(code.contains("ssh ") && code.contains(" -- "));
        assert!(code.contains("docker_path=%s"));
        assert!(code.contains("timeout_path=%s"));
    }
    assert!(
        ssh.contains("-F "),
        "custom SSH config must reach ssh -G and live probes"
    );
    assert!(ssh.contains(r#"const slash = from.lastIndexOf("/")"#));
    assert!(ssh.contains(r#"slash >= 0 ? from.slice(0, slash + 1) : """#));
    assert!(
        !ssh.contains(r#"Math.max(0, from.lastIndexOf("/"))"#),
        "bare config filenames must not prefix relative Includes with the first filename character"
    );
    assert!(ssh.contains("config_file_limit_reached"));
    assert!(ssh.contains("config_truncated"));
    assert!(docker.contains("ssh_config"));
    assert!(
        docker.contains("-F "),
        "one-host inventory must reuse a supplied custom SSH config"
    );
    assert!(aggregate.contains("ssh_config: input.ssh_config"));
    assert!(aggregate.contains("delete artifactInput.ssh_config"));
    assert!(aggregate.contains("parsed_config_file_count"));
    assert!(aggregate.contains("identity_files_configured"));
    assert!(aggregate.contains("artifactTargets"));
    assert!(aggregate.contains("ssh_config_supplied: Boolean(input.ssh_config)"));
}

#[test]
fn repo_status_gh_pulse_builtin_is_discoverable_and_executable() {
    let lab_home = tempfile::tempdir().expect("temp lab home");
    let builtin_dir = builtin_snippet_dir();
    let snippets = list_snippets(lab_home.path(), &builtin_dir).expect("list snippets");
    let info = snippets
        .iter()
        .find(|snippet| snippet.name == "repo-status-gh-pulse")
        .expect("repo-status-gh-pulse listed");

    assert_eq!(info.source, SnippetSource::Builtin);
    assert!(info.inputs.contains_key("owner"));
    assert!(info.inputs.contains_key("repo"));
    assert!(info.inputs.contains_key("root"));
    assert!(info.inputs.contains_key("skill_dir"));

    let resolved = resolve_snippet(lab_home.path(), &builtin_dir, "repo-status-gh-pulse")
        .expect("resolve builtin snippet");
    let code = code_for_snippet(&resolved).expect("extract executable code");

    assert!(code.contains("github::search_pull_requests"));
    assert!(code.contains("claude-macpoo::Bash"));
    assert!(code.contains("github::pull_request_read"));
}

#[test]
fn all_builtin_snippets_satisfy_the_size_and_format_contract() {
    // Guards against the failure mode that silently broke `docs generate`:
    // a built-in tutorial snippet whose body violated the size contract.
    // `collect_snippets` skips invalid snippets silently, so without this
    // test an oversized built-in only surfaces as a `docs generate` abort.
    let dir = builtin_snippet_dir();
    let entries = fs::read_dir(&dir).expect("read builtin snippets directory");
    let mut checked = 0usize;
    for entry in entries {
        let path = entry.expect("builtin snippet dir entry").path();
        if !path.is_file() || !has_snippet_extension(&path) {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("builtin snippet file stem");
        let body = fs::read_to_string(&path).expect("read builtin snippet");
        // Mirror `collect_snippets`' builtin discovery rule: a markdown file
        // with no frontmatter at all (e.g. the directory README) is not a
        // snippet, so it is not held to the snippet contract. A file that
        // *opens* frontmatter (`---`) but fails to parse is a broken builtin
        // and must fail loudly — so skip only on genuine absence and let the
        // `frontmatter(body)?` inside `validate_snippet_body` surface parse
        // errors, rather than collapsing both cases with `.ok().flatten()`.
        if path.extension().and_then(|e| e.to_str()) == Some("md") && !has_frontmatter(&body) {
            continue;
        }
        validate_snippet_body(stem, &body).unwrap_or_else(|error| {
            panic!(
                "builtin snippet `{}` violates the size/format contract: {error}",
                path.display()
            )
        });
        checked += 1;
    }
    assert!(checked > 0, "expected at least one builtin snippet");
}

#[test]
fn validate_snippet_body_bounds_executable_code_not_prose() {
    // A large prose body with a small code block must pass: only the
    // extracted JS is held to MAX_SNIPPET_CODE_BYTES.
    let code = "async () => ({ ok: true })";
    let prose = "x".repeat(MAX_SNIPPET_CODE_BYTES + 4096);
    let body = format!(
        "---\nname: demo\ndescription: Demo snippet\ntags: []\n---\n\n{prose}\n\n```js\n{code}\n```\n"
    );
    assert!(body.len() > MAX_SNIPPET_CODE_BYTES);
    assert!(validate_snippet_body("demo", &body).is_ok());

    // A code block that itself exceeds the code limit must fail.
    let big_code = format!(
        "async () => {{\n{}\nreturn 1;\n}}",
        "x".repeat(MAX_SNIPPET_CODE_BYTES)
    );
    assert!(big_code.len() > MAX_SNIPPET_CODE_BYTES);
    let body = format!(
        "---\nname: demo\ndescription: Demo snippet\ntags: []\n---\n\n```js\n{big_code}\n```\n"
    );
    let error =
        validate_snippet_body("demo", &body).expect_err("oversized code block should be rejected");
    assert!(format!("{error}").contains("snippet code exceeds"));
}

#[test]
fn validate_snippet_body_accepts_context_sized_code_beyond_legacy_20k() {
    let context = "x".repeat(64 * 1024);
    let code = format!(
        "async () => {{ const context = \"{context}\"; return {{ ok: context.length > 0 }}; }}"
    );
    assert!(
        code.len() > 20 * 1024,
        "oracle must exceed the retired 20 KiB cap"
    );
    assert!(
        code.len() < MAX_SNIPPET_CODE_BYTES,
        "oracle should fit the Code Mode source budget"
    );
    assert!(validate_snippet_body("demo", &code).is_ok());
}

#[test]
fn validate_snippet_body_bounds_bare_code_without_fences() {
    // A bare snippet body (no frontmatter, no fences) is its own code, so
    // the code bound governs the whole body directly.
    let small = "async () => ({ ok: true })";
    assert!(validate_snippet_body("demo", small).is_ok());

    let big = format!(
        "async () => {{\n{}\nreturn 1;\n}}",
        "x".repeat(MAX_SNIPPET_CODE_BYTES)
    );
    assert!(big.len() > MAX_SNIPPET_CODE_BYTES);
    let error =
        validate_snippet_body("demo", &big).expect_err("oversized bare code should be rejected");
    assert!(format!("{error}").contains("snippet code exceeds"));
}

#[test]
fn validate_snippet_body_code_bound_is_exclusive_at_the_limit() {
    // Pin the strict `>` comparison: code of exactly MAX_SNIPPET_CODE_BYTES
    // passes, one byte over fails. Guards against a `>` -> `>=` drift.
    let prefix = "async () => { return \"";
    let suffix = "\"; }";
    let pad = MAX_SNIPPET_CODE_BYTES - prefix.len() - suffix.len();
    let at_limit = format!("{prefix}{}{suffix}", "x".repeat(pad));
    assert_eq!(at_limit.len(), MAX_SNIPPET_CODE_BYTES);
    assert!(validate_snippet_body("demo", &at_limit).is_ok());

    let over_limit = format!("{prefix}{}{suffix}", "x".repeat(pad + 1));
    assert_eq!(over_limit.len(), MAX_SNIPPET_CODE_BYTES + 1);
    let error = validate_snippet_body("demo", &over_limit)
        .expect_err("code one byte over the limit should be rejected");
    assert!(format!("{error}").contains("snippet code exceeds"));
}

#[test]
fn validate_snippet_body_rejects_oversized_file() {
    // A file larger than MAX_SNIPPET_FILE_BYTES is rejected before parsing,
    // even though its extracted code would be tiny.
    let prose = "x".repeat(MAX_SNIPPET_FILE_BYTES + 1);
    let body = format!(
        "---\nname: demo\ndescription: Demo snippet\ntags: []\n---\n\n{prose}\n\n```js\nasync () => ({{ ok: true }})\n```\n"
    );
    let error =
        validate_snippet_body("demo", &body).expect_err("oversized file should be rejected");
    assert!(format!("{error}").contains("snippet file exceeds"));
}

#[test]
fn read_resolved_bounds_file_bytes_before_full_read() {
    let dir = tempfile::tempdir().expect("temp snippets");
    let path = dir.path().join("demo.js");
    fs::write(&path, vec![b'x'; MAX_SNIPPET_FILE_BYTES + 4096]).expect("write oversized fixture");

    let error = read_resolved("demo", SnippetSource::User, path)
        .expect_err("oversized on-disk snippet must fail before a full read");
    assert!(format!("{error}").contains("snippet file exceeds"));
}

#[test]
fn read_resolved_rejects_non_utf8_snippet_files() {
    let dir = tempfile::tempdir().expect("temp snippets");
    let path = dir.path().join("demo.js");
    fs::write(&path, [0xff, 0xfe, 0xfd]).expect("write non-UTF8 fixture");

    let error = read_resolved("demo", SnippetSource::User, path)
        .expect_err("saved snippets are UTF-8 text");
    assert!(format!("{error}").contains("valid UTF-8"));
}

#[test]
fn merge_snippet_input_rejects_unknown_declared_inputs() {
    let body = "---\nname: demo\ndescription: Demo snippet\ninputs:\n  host:\n    type: string\n    default: node-a\n---\n\n```js\nasync (input) => input\n```\n";
    let metadata = frontmatter(body)
        .expect("frontmatter parsed")
        .expect("metadata");
    let snippet = ResolvedSnippet {
        content_digest: None,
        tools: metadata.tools,
        name: "demo".to_string(),
        description: Some(metadata.description),
        tags: metadata.tags,
        inputs: metadata.inputs,
        source: SnippetSource::User,
        path: PathBuf::from("demo.md"),
        body: body.to_string(),
    };

    let error = merge_snippet_input(&snippet, json!({"bogus": true}))
        .expect_err("unknown input should be rejected");
    assert!(format!("{error}").contains("unknown snippet input"));
}
