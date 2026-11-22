//! Runtime JS proxy generation for Code Mode.
//!
//! Generates the executable `var codemode = {...}` proxy object that is sent
//! through the Code Mode Start protocol and injected into the sandbox after
//! `callTool` is defined. The proxy lets the agent call
//! `codemode.<namespace>.<tool>(params)`, which routes to
//! `callTool("<namespace>::<dotted.name>", params)`.
//!
//! This is RUNTIME JS, not a TypeScript declaration: it never enters the
//! model's context (unlike the deleted typed-preamble-in-tool-description
//! machinery from commit 780c67d3). Surfacing types/schemas via `search` is a
//! separate follow-up; this module only restores the executable proxy.

mod discovery;
#[cfg(test)]
mod discovery_tests;
mod providers;
mod proxy;
#[cfg(test)]
mod proxy_tests;
#[cfg(test)]
mod snippet_declaration_tests;
#[cfg(test)]
mod withheld_tests;

use crate::host::WithheldTools;
#[cfg(test)]
pub(crate) use discovery::generate_discovery_js;
pub(crate) use discovery::generate_discovery_js_from_json;
pub(crate) use providers::{generate_local_provider_js, generate_openapi_provider_js};
pub(crate) use proxy::generate_js_proxy_from_catalog;

// ────────────────────────────────────────────────────────────────────────────
// Tool name conversion (snake_case — Cloudflare Code Mode parity)
// ────────────────────────────────────────────────────────────────────────────
//
// Cloudflare's Code Mode normalizes tool ids like `my-server.list-items` to
// `my_server_list_items` (all separators → `_`). We do the same so that an
// LLM trained on Cloudflare examples calls the right names.

/// JavaScript reserved words that need an underscore suffix.
const JS_RESERVED: &[&str] = &[
    "abstract",
    "arguments",
    "await",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "function",
    "goto",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "int",
    "interface",
    "let",
    "long",
    "native",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "volatile",
    "while",
    "with",
    "yield",
];

/// Top-level `codemode` helper names owned by Lab's local runtime.
///
/// Namespaces that sanitize to one of these names are suffixed so a
/// real namespace named `search`, `describe`, `step`, or `batch` cannot overwrite the
/// local discovery/control helpers.
const CODEMODE_TOP_LEVEL_RESERVED: &[&str] = &[
    "search",
    "describe",
    "listResources",
    "readResource",
    "readArtifact",
    "callToolResult",
    "artifactInfo",
    "listArtifacts",
    "getPrompt",
    "listSkills",
    "getSkill",
    "readSkill",
    "run",
    "step",
    "batch",
];

/// Convert a dotted/hyphenated/slashed/coloned tool name to snake_case.
///
/// Examples (Cloudflare parity):
/// - `status.get` → `status_get`
/// - `tv-show.get` → `tv_show_get`
/// - `create/issue` → `createissue`
/// - `list:repos` → `listrepos`
/// - `delete` → `delete_` (reserved word)
/// - `2fa_setup` → `_2fa_setup` (leading digit prefixed with `_`)
///
/// `status.get` and `status_get` both map to `status_get`. Proxy generation
/// omits colliding normalized helpers, including final namespace/tool keys,
/// rather than choosing one tool implicitly.
pub(crate) fn tool_name_to_snake(name: &str) -> String {
    if name.is_empty() {
        return "_".to_string();
    }

    // Cloudflare parity: replace common separators with underscores, then strip
    // any remaining characters that are invalid in JavaScript identifiers.
    let mut snake = String::new();
    let mut previous_was_separator = false;
    for ch in name.chars() {
        if ch == '-' || ch == '.' || ch.is_whitespace() {
            if !previous_was_separator {
                snake.push('_');
            }
            previous_was_separator = true;
        } else if ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            snake.push(ch);
            previous_was_separator = false;
        } else {
            previous_was_separator = false;
        }
    }
    let snake = snake.trim_matches('_').to_string();
    let snake = if snake.is_empty() {
        "_".to_string()
    } else {
        snake
    };

    // Prefix with `_` if the result starts with a digit (invalid JS identifier start).
    let snake = if snake.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{snake}")
    } else {
        snake
    };

    if JS_RESERVED.contains(&snake.as_str()) {
        format!("{snake}_")
    } else {
        snake
    }
}

/// Convert a namespace name to the runtime `codemode.<namespace>` key.
pub(crate) fn namespace_segment(name: &str) -> String {
    let namespace = tool_name_to_snake(name);
    if CODEMODE_TOP_LEVEL_RESERVED.contains(&namespace.as_str()) {
        format!("{namespace}_")
    } else {
        namespace
    }
}

// ────────────────────────────────────────────────────────────────────────────
// JS proxy generation (runtime executable, not type declarations)
// ────────────────────────────────────────────────────────────────────────────

/// Agent-facing explanation for tools a read-only run withheld.
/// How to reach tools a read-only run withholds. Shared by every withheld
/// message (Rust per-upstream guidance and the JS multi-upstream hints).
const WITHHELD_SCOPE_ADVICE: &str = "Use the `codemode` tool to reach them (requires the `lab` or `lab:admin` scope); if this client only holds `lab:read`, reconnect it with the `lab` scope.";

pub(crate) fn withheld_guidance(withheld: &WithheldTools) -> String {
    let count = withheld.tool_count();
    let noun = if count == 1 { "tool" } else { "tools" };
    format!(
        "{count} {noun} from upstream `{namespace}` are hidden because this is a read-only Code Mode run (`codemode_read`) and the upstream does not annotate them `readOnlyHint: true`. {WITHHELD_SCOPE_ADVICE}",
        namespace = withheld.namespace()
    )
}
