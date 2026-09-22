//! Model-visible descriptions for the Code Mode entry points.
//!
//! MCP clients commonly display only the first ~2 KB of a tool description,
//! so the order is deliberate: the line that distinguishes this entry point
//! (read-only vs write-capable), then the rules that cause the most real
//! failures, then one concrete example, then the upstream list. Detail that
//! the runtime already delivers when it matters (error `recovery` guidance,
//! truncation markers, `codemode.describe()` declarations) stays out.
//! Long-form reference lives in `docs/dev/CODE_MODE.md`.

use serde_json::Value;

/// Hard cap on the rendered description.
pub(crate) const CODE_MODE_DESCRIPTION_MAX_BYTES: usize = 8192;

/// Most required parameters rendered into a generated example call.
const EXAMPLE_MAX_PARAMS: usize = 3;

/// Longest upstream-supplied tool or parameter name rendered into the
/// description. Upstream text lands in the client-visible prefix, so it is
/// bounded and restricted to identifier-like characters.
const EXAMPLE_MAX_NAME_BYTES: usize = 64;

/// Which Code Mode entry point a description is rendered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeModeDescriptionVariant {
    /// `codemode`: write-capable execution.
    Full,
    /// `codemode_read`: only tools annotated `readOnlyHint: true`.
    Read,
    /// `codemode_ui`: write-capable execution rendered as a trace inspector.
    Ui,
}

/// One enabled, route-visible upstream rendered in the namespace list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeModeUpstreamDescription {
    pub(crate) name: String,
    pub(crate) hint: Option<String>,
    /// A real tool from this upstream used to render the example call.
    pub(crate) example: Option<CodeModeExampleCall>,
}

/// A concrete upstream call rendered as the description's example.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeModeExampleCall {
    pub(crate) tool: String,
    /// JS object literal for the tool's required parameters.
    pub(crate) args: String,
    /// Whether the tool is explicitly read-only (eligible for `codemode_read`).
    pub(crate) read_only: bool,
}

impl CodeModeExampleCall {
    /// Build an example from a tool's input schema: its first few required
    /// parameters, each with a `"<name>"` placeholder that is obviously not a
    /// real value. Only upstream *names* are rendered, never enum values or
    /// descriptions, and only when they are short and identifier-like.
    /// Returns `None` when the tool is not usable as an example.
    pub(crate) fn from_tool(
        tool: &str,
        input_schema: &serde_json::Map<String, Value>,
        read_only: bool,
    ) -> Option<Self> {
        if !read_only || !example_safe_name(tool, &['_', '-', '.']) {
            return None;
        }
        let required = input_schema
            .get("required")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .take(EXAMPLE_MAX_PARAMS)
            .collect::<Vec<_>>();
        if !required
            .iter()
            .all(|name| example_safe_name(name, &['_', '-', '$']))
        {
            return None;
        }
        let fields = required
            .iter()
            .map(|name| format!("{}: \"<{name}>\"", js_key(name)))
            .collect::<Vec<_>>();
        let args = if fields.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {} }}", fields.join(", "))
        };
        Some(Self {
            tool: tool.to_string(),
            args,
            read_only,
        })
    }
}

fn example_safe_name(name: &str, extra: &[char]) -> bool {
    !name.is_empty()
        && name.len() <= EXAMPLE_MAX_NAME_BYTES
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || extra.contains(&ch))
}

fn js_key(name: &str) -> String {
    let identifier = name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_' || ch == '$')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$');
    if identifier {
        name.to_string()
    } else {
        format!("\"{name}\"")
    }
}

fn header(variant: CodeModeDescriptionVariant) -> &'static str {
    match variant {
        CodeModeDescriptionVariant::Read => {
            "Read-only Code Mode: run JavaScript that calls only upstream MCP tools annotated \
`readOnlyHint: true`. Nothing can be changed; `writeArtifact` calls are rejected. To change \
state, use `codemode` (requires the `lab` scope). `codemode.search()` reports tools hidden \
by this restriction."
        }
        CodeModeDescriptionVariant::Full => {
            "Write-capable Code Mode: run JavaScript that can call any upstream MCP tool, \
including ones that change state. Use `codemode_read` when you only need to read."
        }
        CodeModeDescriptionVariant::Ui => {
            "Code Mode with a visual trace inspector: same write-capable execution as \
`codemode`, rendered as an MCP App. Use `codemode` when nested upstream MCP Apps should \
become the active result UI."
        }
    }
}

const CORE: &str = "\
Pass `code` as `async () => { ... }`; its return value is the result.

Workflow:
1. Find: `(await codemode.search({ query: \"short intent\", limit: 5 })).results` gives each \
tool's `path`, `id`, `helper`, and `signature`.
2. Check (only if the signature is not enough): return `await codemode.describe(path)` and call \
in the next run; output only reaches you if the script returns it.
3. Call the `helper` as given (e.g. `codemode.my_server.get_issue(params)`; `-`/`.` become \
`_`), or `callTool(\"upstream::tool\", params)` with raw names.
Never guess tool or parameter names.

Rules:
- Return only what is needed: select fields, slice arrays. Results over the budget \
(24 KB default) are truncated.
- Defaults: ~30 s and 512 tool calls per run. Fan out with `codemode.batch([() => ..., \
() => ...])`, not `Promise.all`; it never rejects and resolves `{ ok, failed, all_ok }`.
- A failed call rejects only its own promise. Catch it and return the error: its \
`recovery.guidance` and `side_effects` say whether and how to retry.";

const WRITE_RULE: &str = "\
- A timeout does not undo completed writes. Check what finished and reuse idempotency keys \
before retrying.";

const TAIL_RULES: &str = "\
- No `fetch`, `fs`, `require`, or Node APIs; all I/O goes through tools. Labby's own \
services (e.g. `gateway`) are separate MCP tools, not callable here.
- Optional inputs `upstreams` and `tools` narrow the run; upstream names ignore case and `-`/`_`.";

fn globals_line(variant: CodeModeDescriptionVariant) -> &'static str {
    match variant {
        CodeModeDescriptionVariant::Read => "Globals: `codemode`, `callTool`.",
        CodeModeDescriptionVariant::Full | CodeModeDescriptionVariant::Ui => {
            "Globals: `codemode`, `callTool`, `writeArtifact`."
        }
    }
}

fn example_block(upstreams: &[CodeModeUpstreamDescription]) -> String {
    // Models copy the example verbatim, so it is only ever a read-only tool
    // (enforced when the example is built) with placeholder arguments.
    let example = upstreams.iter().find_map(|upstream| {
        upstream
            .example
            .as_ref()
            .filter(|example| example.read_only)
            .map(|example| (upstream.name.as_str(), example))
    });
    match example {
        Some((upstream, example)) => {
            let id = serde_json::to_string(&format!("{upstream}::{}", example.tool))
                .unwrap_or_else(|_| "\"\"".to_string());
            format!(
                "Example (read-only; replace each <placeholder>):\n```js\nasync () => {{\n  \
const result = await callTool({id}, {args});\n  return result; // select only the fields you \
need\n}}\n```",
                args = example.args
            )
        }
        None => "Example:\n```js\nasync () => {\n  const hits = await codemode.search({ query: \
\"list open issues\", limit: 5 });\n  return hits.results.map(r => ({ helper: r.helper, \
signature: r.signature }));\n}\n```"
            .to_string(),
    }
}

fn upstream_line(upstream: &CodeModeUpstreamDescription) -> String {
    match upstream
        .hint
        .as_deref()
        .and_then(labby_runtime::gateway_config::normalize_code_mode_hint)
    {
        Some(hint) => format!("- `{}`: {hint}", upstream.name),
        None => format!("- `{}`", upstream.name),
    }
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// Render the description for one Code Mode entry point.
///
/// The fixed part (header, rules, example) always survives. Upstream lines
/// are added whole until the cap, then an overflow line points at
/// `codemode.search()`. `trailer` is optional low-priority guidance appended
/// only when it fits.
#[must_use]
pub(crate) fn code_mode_tool_description(
    variant: CodeModeDescriptionVariant,
    upstreams: &[CodeModeUpstreamDescription],
    trailer: &str,
) -> String {
    let mut rules = String::from(CORE);
    if variant != CodeModeDescriptionVariant::Read {
        rules.push('\n');
        rules.push_str(WRITE_RULE);
    }
    rules.push('\n');
    rules.push_str(TAIL_RULES);

    let mut out = format!(
        "{}\n\n{rules}\n\n{}\n\n{}\n\n## Upstreams\n",
        header(variant),
        globals_line(variant),
        example_block(upstreams)
    );

    let trailer = trailer.trim();
    let trailer_bytes = if trailer.is_empty() {
        0
    } else {
        trailer.len() + 2
    };
    if upstreams.is_empty() {
        out.push_str("- none currently configured");
    } else {
        for (index, upstream) in upstreams.iter().enumerate() {
            let line = upstream_line(upstream);
            let remaining = upstreams.len() - index;
            let overflow =
                format!("- ...and {remaining} more; use `codemode.search()` to discover them");
            let is_last = remaining == 1;
            let reserve = if is_last { 0 } else { overflow.len() + 1 };
            if out.len() + line.len() + 1 + reserve > CODE_MODE_DESCRIPTION_MAX_BYTES {
                out.push_str(&overflow);
                out.push('\n');
                break;
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    let mut out = out.trim_end().to_string();
    if trailer_bytes > 0 && out.len() + trailer_bytes <= CODE_MODE_DESCRIPTION_MAX_BYTES {
        out.push_str("\n\n");
        out.push_str(trailer);
    }
    utf8_prefix(&out, CODE_MODE_DESCRIPTION_MAX_BYTES).to_string()
}
