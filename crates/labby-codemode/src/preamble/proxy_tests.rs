use super::*;
use crate::types::CatalogDescriptor;
/// Build a `CatalogDescriptor` for the proxy-generation tests.
fn descriptor(namespace: &str, tool: &str) -> CatalogDescriptor {
    CatalogDescriptor::tool(namespace, tool, "", None, None)
}

#[test]
fn openapi_shim_is_valid_js() {
    let js = generate_openapi_provider_js();
    assert!(js.contains("globalThis.openapi") && js.contains("call"));
    assert!(js.contains("function (label, operationId, params)"));
    assert!(js.contains(r#"callTool("openapi::" + label + "." + operationId"#));
}

/// Generate the runtime proxy from owned descriptors.
fn proxy(tools: &[CatalogDescriptor]) -> Result<String, String> {
    let refs: Vec<&CatalogDescriptor> = tools.iter().collect();
    generate_js_proxy_from_catalog(&refs)
}

// ── tool_name_to_snake ────────────────────────────────────────────────────

#[test]
fn tool_name_to_snake_converts_dotted_names() {
    // PRESENCE: basic dotted/hyphenated name conversion (Cloudflare parity)
    assert_eq!(tool_name_to_snake("status.get"), "status_get");
    assert_eq!(tool_name_to_snake("tv-show.get"), "tv_show_get");
    // PRESENCE: reserved word gets underscore suffix
    assert_eq!(tool_name_to_snake("delete"), "delete_");
    // ABSENCE: separators must not appear in snake output
    assert!(!tool_name_to_snake("status.get").contains('.'));
    assert!(!tool_name_to_snake("tv-show.get").contains('-'));
}

#[test]
fn tool_name_to_snake_handles_slashes_and_colons() {
    // PRESENCE: forward-slashes and colons join with underscore
    assert_eq!(tool_name_to_snake("create/issue"), "createissue");
    assert_eq!(tool_name_to_snake("list:repos"), "listrepos");
    assert_eq!(
        tool_name_to_snake("repos/create/branch"),
        "reposcreatebranch"
    );
    // PRESENCE: already-snake input is preserved
    assert_eq!(tool_name_to_snake("create_issue"), "create_issue");
    // PRESENCE: leading digit gets prefixed with underscore
    assert_eq!(tool_name_to_snake("2fa_setup"), "_2fa_setup");
    // ABSENCE: separators must not appear in output (would break JS syntax)
    assert!(!tool_name_to_snake("create/issue").contains('/'));
    assert!(!tool_name_to_snake("list:repos").contains(':'));
}

#[test]
fn tool_name_to_snake_matches_cloudflare_identifier_sanitization() {
    assert_eq!(tool_name_to_snake("list tags"), "list_tags");
    assert_eq!(tool_name_to_snake("create#issue!"), "createissue");
    assert_eq!(tool_name_to_snake("await"), "await_");
    assert_eq!(tool_name_to_snake(""), "_");
}

#[test]
fn namespace_segment_preserves_top_level_helpers() {
    assert_eq!(namespace_segment("github"), "github");
    assert_eq!(namespace_segment("search"), "search_");
    assert_eq!(namespace_segment("describe"), "describe_");
    assert_eq!(namespace_segment("step"), "step_");
    assert_eq!(namespace_segment("batch"), "batch_");
}

#[test]
fn generate_js_proxy_quoted_keys_tolerate_special_chars() {
    // Tool with a slash in the name — previously caused a QuickJS syntax
    // error because the unquoted key was invalid.
    let tool = descriptor("github", "create/issue");
    let js = proxy(&[tool]).expect("proxy");

    // PRESENCE: the namespace object must be present
    assert!(
        js.contains("codemode[\"github\"]"),
        "namespace block missing"
    );
    // PRESENCE: the tool key must appear as a quoted string (not unquoted)
    assert!(
        js.contains("\"createissue\""),
        "sanitized key must be quoted"
    );
    // ABSENCE: no unquoted slash-containing key that would break JS syntax
    assert!(
        !js.contains("create/issue:"),
        "slash in unquoted key would break JS"
    );
    // PRESENCE: callTool must be wired to the original dotted tool id
    assert!(
        js.contains("github::create/issue"),
        "original tool id must be preserved"
    );
}

#[test]
fn generate_js_proxy_emits_codemode_global_and_method() {
    let tool = descriptor("gateway-alpha", "status.get");
    let js = proxy(&[tool]).expect("proxy");

    // PRESENCE: preserves the platform-created codemode object
    assert!(
        js.contains("globalThis.codemode = globalThis.codemode || {}"),
        "must preserve codemode object"
    );
    assert!(
        js.contains("var codemode = globalThis.codemode"),
        "must bind preserved codemode object"
    );
    assert!(
        !js.contains("var codemode = {}"),
        "must not replace codemode"
    );
    // PRESENCE: snake_case method routes to the dotted tool id
    assert!(
        js.contains("gateway-alpha::status.get"),
        "method must route to dotted tool id"
    );
    // PRESENCE: null-safe params guard (no nullish-coalescing dependency)
    assert!(
        js.contains("p == null ? {} : p"),
        "must use null-safe params guard"
    );
    // ABSENCE: must not use nullish-coalescing (engine-portability)
    assert!(!js.contains("?? {}"), "must not depend on ?? operator");
}

#[test]
fn generate_js_proxy_does_not_overwrite_local_discovery_helpers() {
    for raw in [
        "search",
        "describe",
        "listResources",
        "readResource",
        "getPrompt",
        "listSkills",
        "getSkill",
        "readSkill",
        "step",
        "batch",
    ] {
        let namespace = namespace_segment(raw);
        let tool = descriptor(raw, "lookup");
        let js = proxy(&[tool]).expect("proxy");

        assert!(
            js.contains(&format!("codemode[\"{namespace}\"]")),
            "reserved namespace must be suffixed: {js}"
        );
        assert!(
            !js.contains(&format!("codemode[\"{raw}\"] = {{")),
            "reserved namespace must not replace codemode.{raw}"
        );
        assert!(
            js.contains(&format!("{raw}::lookup")),
            "raw id must still route to the original namespace"
        );
    }
}

#[test]
fn generate_js_proxy_snake_cases_hyphenated_namespace_keys() {
    // Hyphenated namespaces (arcane-mcp, github-chat, …) must be reachable
    // via dot notation `codemode.arcane_mcp.tool(...)`; the callTool id keeps
    // the RAW namespace name so the host routes to the real source.
    let tool = descriptor("arcane-mcp", "arcane");
    let js = proxy(&[tool]).expect("proxy");

    assert!(
        js.contains("codemode[\"arcane_mcp\"]"),
        "hyphenated namespace key must be snake_cased: {js}"
    );
    // The raw name is only an alias of the snake-cased object, for
    // agents that copy the configured upstream name verbatim.
    assert!(
        js.contains(
            "if (!Object.prototype.hasOwnProperty.call(codemode, \"arcane-mcp\")) codemode[\"arcane-mcp\"] = codemode[\"arcane_mcp\"];"
        ),
        "raw hyphenated key must alias the snake-cased object: {js}"
    );
    assert!(
        !js.contains("codemode[\"arcane-mcp\"] = {"),
        "raw key must not get its own proxy object: {js}"
    );
    assert!(
        js.contains("arcane-mcp::arcane"),
        "callTool id must keep the raw namespace name: {js}"
    );
}

#[test]
fn generate_js_proxy_rejects_sanitized_tool_collisions() {
    let dotted = descriptor("demo", "status.get");
    let underscored = descriptor("demo", "status_get");

    let healthy = descriptor("other", "ok");
    let js = proxy(&[dotted, underscored, healthy]).unwrap();
    assert!(js.contains("other::ok"));
    assert!(!js.contains("demo::status"));
}

#[test]
fn generate_js_proxy_rejects_final_proxy_collisions_across_raw_namespaces() {
    let hyphenated = descriptor("foo-bar", "ping");
    let dotted = descriptor("foo.bar", "ping");

    let js = proxy(&[hyphenated, dotted]).unwrap();
    assert!(!js.contains("::ping"));
}
