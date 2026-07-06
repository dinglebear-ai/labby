//! User-code normalization for the Code Mode sandbox.

/// Normalize user-submitted code before sandbox execution.
///
/// The execute wrapper evaluates `code` as a FUNCTION EXPRESSION
/// (`const __codeModeMain = ({code}); ... return await __codeModeMain();`), so
/// every tolerated input shape must reduce to a *bare parenthesized function
/// expression with no trailing invocation*. A self-invoking IIFE or a trailing
/// `main();` call would break the wrapper (the grouping would contain a Promise
/// or two statements). Transforms:
/// 1. Strip markdown fences (```javascript/typescript/``` wrappers).
/// 2. Bare `function main` / `async function main` declarations are wrapped and
///    called from an async arrow.
/// 3. `export default [async] function` → strip `export default ` and
///    parenthesize the function expression — NO trailing IIFE `()`.
/// 4. A bare arrow `async () => {...}` passes through unchanged (it is already a
///    function expression).
/// 5. Loose statements / trailing expressions are wrapped in `async () => { ... }`;
///    if the trailing statement looks like an expression, it is returned.
/// 6. `export default <X>` preceded by prologue statements keeps the prologue
///    and invokes the default-export entry so it closes over those bindings.
///
/// Only `execute` normalizes the caller's code through this before handing it to
/// the Javy runner. `search` passes its code to the runner *raw* (no
/// normalization) so that a non-function search input still surfaces as a
/// contract error instead of being silently wrapped into a valid async arrow.
///
/// Exposed (`pub`) so integration tests can normalize a body form and pipe the
/// exact post-normalize string through the runner end to end.
pub fn normalize_user_code(code: &str) -> String {
    let code = strip_code_fences(code.trim()).trim();
    if code.is_empty() {
        return "async () => {}".to_string();
    }
    if let Some(inner) = code.strip_prefix("export default ") {
        let inner = inner.trim().trim_end_matches(';').trim();
        if inner.starts_with("async function") || inner.starts_with("function") {
            return format!("async () => {{\nreturn ({inner})();\n}}");
        }
        if inner.starts_with("class") {
            return format!("async () => {{\nreturn ({inner});\n}}");
        }
        return normalize_user_code(inner);
    }
    if let Some(name) = function_declaration_name(code) {
        return format!("async () => {{\n{code}\nreturn {name}();\n}}");
    }
    if is_bare_function_expression(code) || is_bare_arrow_expression(code) {
        return strip_trailing_statement_semicolon(code);
    }
    if let Some((prologue, value)) = split_prologue_export_default(code) {
        let value = strip_trailing_named_exports(value)
            .trim()
            .trim_end_matches(';')
            .trim();
        if !value.is_empty() {
            let prologue = strip_prologue_exports(prologue);
            let entry = normalize_user_code(&format!("export default {value}"));
            return format!("async () => {{\n{prologue}\nreturn await ({entry})();\n}}");
        }
    }
    wrap_loose_code_as_async_arrow(code)
}

/// Split `{prologue} export default {value}` into the prologue and the
/// default-export value, but only when `export default` appears at a statement
/// boundary after a non-empty prologue (the prologue ends at a `;` or `}`).
///
/// This is a conservative textual fallback. String-literal safety does NOT come
/// from this function — it comes from the caller: this runs only after both the
/// module and script parses fail, so any valid script that merely contains
/// `; export default` inside a string parses as a script first and never reaches
/// here (see `normalize_user_code` and the `..._inside_a_string` test). The
/// start-anchored `export default` case is also handled earlier, so this only
/// fires when a real prologue precedes an otherwise-unparseable arrow default.
fn split_prologue_export_default(code: &str) -> Option<(&str, &str)> {
    const NEEDLE: &str = "export default ";
    for idx in export_default_indices(code, NEEDLE) {
        let before = code[..idx].trim_end();
        // A real statement terminator wins as-is. Only when `before` does not
        // already end at a boundary do we retry after stripping a trailing
        // comment, so `const x = 1; // note\nexport default ...` still splits —
        // without letting a `//` *inside* a prologue string (e.g. a "http://"
        // URL) corrupt an otherwise-terminated prologue.
        let ends_at_boundary = |s: &str| s.ends_with(';') || s.ends_with('}');
        // `strip_trailing_comment` bails on any earlier `//` (it can't tell a
        // string-internal `//` from a comment). That defeats a prologue whose
        // tail holds both a "http://" URL string and a real trailing comment.
        // `strip_suffix_line_comment` inspects only the last `//` on the final
        // line, so it strips the genuine trailing comment regardless. A spurious
        // strip can only split here if the text before that `//` already ends at
        // a `;`/`}` — gated by `ends_at_boundary` and by this running only after
        // both real parses failed.
        let comment_stripped = strip_trailing_comment(before).trim_end();
        let suffix_stripped = strip_suffix_line_comment(before);
        if (!before.is_empty() && ends_at_boundary(before))
            || (!comment_stripped.is_empty() && ends_at_boundary(comment_stripped))
            || (!suffix_stripped.is_empty() && ends_at_boundary(suffix_stripped))
        {
            return Some((before, &code[idx + NEEDLE.len()..]));
        }
    }
    None
}

/// Iterate over `code`'s character positions annotated with whether that
/// position is in normal code (vs inside a string/template literal or a
/// comment), so callers can safely pattern-match braces/needles without being
/// fooled by an identical character sitting inside a string or comment.
///
/// Shared by [`export_default_indices`] (needle search) and
/// [`matching_brace_end`] (brace-depth tracking) — both need the same
/// string/comment-aware walk, just a different thing to do with each
/// in-code character.
fn scan_code_positions(code: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Code,
        Single,
        Double,
        Template,
        LineComment,
        BlockComment,
    }

    let mut hits = Vec::new();
    let mut state = State::Code;
    let mut escaped = false;
    let mut iter = code.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        match state {
            State::Code => {
                hits.push((idx, ch));
                match ch {
                    '\'' => state = State::Single,
                    '"' => state = State::Double,
                    '`' => state = State::Template,
                    '/' if iter.peek().is_some_and(|(_, next)| *next == '/') => {
                        iter.next();
                        state = State::LineComment;
                    }
                    '/' if iter.peek().is_some_and(|(_, next)| *next == '*') => {
                        iter.next();
                        state = State::BlockComment;
                    }
                    _ => {}
                }
            }
            State::Single => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '\'' {
                    state = State::Code;
                }
            }
            State::Double => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    state = State::Code;
                }
            }
            State::Template => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '`' {
                    state = State::Code;
                }
            }
            State::LineComment => {
                if ch == '\n' {
                    state = State::Code;
                }
            }
            State::BlockComment => {
                if ch == '*' && iter.peek().is_some_and(|(_, next)| *next == '/') {
                    iter.next();
                    state = State::Code;
                }
            }
        }
    }
    hits.into_iter()
}

fn export_default_indices<'a>(code: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    scan_code_positions(code)
        .filter(move |(idx, _)| code[*idx..].starts_with(needle))
        .map(|(idx, _)| idx)
}

fn wrap_loose_code_as_async_arrow(code: &str) -> String {
    let code = code.trim().trim_end_matches(';').trim();
    if code.is_empty() {
        return "async () => {}".to_string();
    }
    if let Some((before, after)) = code.rsplit_once(';') {
        let trailing = after.trim();
        if !trailing.is_empty() && looks_like_returnable_expression(trailing) {
            return format!("async () => {{\n{before};\nreturn ({trailing})\n}}");
        }
    } else if looks_like_returnable_expression(code) && !code.trim_start().starts_with("return ") {
        return format!("async () => {{\nreturn ({code})\n}}");
    }

    format!("async () => {{\n{code}\n}}")
}

fn strip_code_fences(code: &str) -> &str {
    let trimmed = code.trim();
    for lang in ["javascript", "typescript", "tsx", "jsx", "js", "ts", ""] {
        let prefix = if lang.is_empty() {
            "```\n".to_string()
        } else {
            format!("```{lang}\n")
        };
        if let Some(stripped) = trimmed.strip_prefix(&prefix)
            && let Some(inner) = stripped.strip_suffix("```")
        {
            return inner.trim();
        }
    }
    trimmed
}

fn strip_prologue_exports(prologue: &str) -> String {
    let mut out = Vec::new();
    for line in prologue.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import ") || trimmed.starts_with("export {") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("export ") {
            let indent_len = line.len() - trimmed.len();
            out.push(format!("{}{}", &line[..indent_len], rest));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn strip_trailing_named_exports(value: &str) -> &str {
    for marker in ["\nexport ", "\r\nexport "] {
        if let Some((before, _)) = value.split_once(marker) {
            return before;
        }
    }
    value
}

fn strip_trailing_statement_semicolon(source: &str) -> String {
    // Strip a trailing line/block comment first so `async () => 42; // note`
    // does not leave a `;` (and the comment) inside the wrapper grouping, which
    // would be a syntax error. After removing any trailing comment + whitespace,
    // drop the statement-terminating semicolon.
    let trimmed = strip_trailing_comment(source.trim_end()).trim_end();
    trimmed.strip_suffix(';').map_or_else(
        || trimmed.to_string(),
        |without| without.trim_end().to_string(),
    )
}

/// Strip a trailing `// ...` line comment, inspecting only the last `//` on the
/// final line. Unlike [`strip_trailing_comment`], this tolerates an earlier `//`
/// elsewhere in `source` (e.g. inside a `"http://..."` URL string in a prologue).
///
/// Only [`split_prologue_export_default`] uses this, where the result feeds a
/// `;`/`}` boundary check — a string-internal last `//` produces a non-boundary
/// remainder and is rejected there, so this loose strip is safe in that context
/// but is NOT a general-purpose comment stripper.
fn strip_suffix_line_comment(source: &str) -> &str {
    let trimmed = source.trim_end();
    let line_start = trimmed.rfind('\n').map_or(0, |i| i + 1);
    match trimmed[line_start..].rfind("//") {
        Some(rel) => trimmed[..line_start + rel].trim_end(),
        None => trimmed,
    }
}

/// Remove a single trailing `// ...` line comment or `/* ... */` block comment
/// (and only when it is genuinely at the end of the source). Conservative: bails
/// out unchanged if a `//` or `*/` also appears earlier, since a mid-source
/// occurrence may be inside a string literal we must not disturb.
fn strip_trailing_comment(source: &str) -> &str {
    let trimmed = source.trim_end();
    if let Some(start) = trailing_comment_start(trimmed) {
        return trimmed[..start].trim_end();
    }
    trimmed
}

fn trailing_comment_start(source: &str) -> Option<usize> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Code,
        Single,
        Double,
        Template,
        LineComment(usize),
        BlockComment(usize),
    }

    let mut state = State::Code;
    let mut escaped = false;
    let mut iter = source.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        match state {
            State::Code => match ch {
                '\'' => state = State::Single,
                '"' => state = State::Double,
                '`' => state = State::Template,
                '/' if iter.peek().is_some_and(|(_, next)| *next == '/') => {
                    iter.next();
                    state = State::LineComment(idx);
                }
                '/' if iter.peek().is_some_and(|(_, next)| *next == '*') => {
                    iter.next();
                    state = State::BlockComment(idx);
                }
                _ => {}
            },
            State::Single => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '\'' {
                    state = State::Code;
                }
            }
            State::Double => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    state = State::Code;
                }
            }
            State::Template => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '`' {
                    state = State::Code;
                }
            }
            State::LineComment(start) => {
                if ch == '\n' {
                    state = State::Code;
                } else if iter.peek().is_none() {
                    return Some(start);
                }
            }
            State::BlockComment(start) => {
                if ch == '*' && iter.peek().is_some_and(|(_, next)| *next == '/') {
                    iter.next();
                    if iter.peek().is_none() {
                        return Some(start);
                    }
                    state = State::Code;
                }
            }
        }
    }

    match state {
        State::LineComment(start) => Some(start),
        _ => None,
    }
}

/// Returns the function's name, but ONLY when `code` is a *bare* declaration —
/// i.e. the declaration's body brace-balances all the way to the end of
/// `code` (aside from trailing whitespace/semicolons). Without this check, a
/// prologue like `function mk() {...}\nconst tool = mk();\nexport default
/// ...;` would be misidentified as a single wrapped declaration (matching
/// only the `function ` prefix) and its trailing statements silently
/// swallowed into a bogus `return mk();` — the multi-statement prologue path
/// (`split_prologue_export_default`) must handle that case instead.
fn function_declaration_name(code: &str) -> Option<&str> {
    let code = code.trim_start();
    let rest = code
        .strip_prefix("async function ")
        .or_else(|| code.strip_prefix("function "))?;
    let name = rest.split_once('(')?.0.trim();
    if name.is_empty() {
        return None;
    }
    let body_start = code.find('{')?;
    let body_end = matching_brace_end(code, body_start)?;
    code[body_end..]
        .trim()
        .trim_end_matches(';')
        .trim()
        .is_empty()
        .then_some(name)
}

/// Given the byte index of an opening `{`, returns the index just past its
/// matching `}`, tracking string/template/comment state so braces inside
/// those do not throw off the depth count.
/// Given the byte offset of an opening `{` in `code`, returns the offset just
/// past its matching `}`, using [`scan_code_positions`] so braces inside a
/// string/template literal or a comment don't throw off the depth count.
fn matching_brace_end(code: &str, open_brace: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    for (offset, ch) in scan_code_positions(&code[open_brace..]) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open_brace + offset + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `code` IN ITS ENTIRETY is a single function expression (named or
/// anonymous) — i.e. the function's body brace-balances all the way to the
/// end of `code`. A prefix match alone would misidentify a prologue like
/// `function mk() {...}\nconst tool = mk();` (a function decl followed by
/// more statements) as a bare expression and silently drop everything after
/// it — see [`function_declaration_name`] for the same failure mode.
fn is_bare_function_expression(code: &str) -> bool {
    let code = code.trim_start();
    if !(code.starts_with("async function") || code.starts_with("function")) {
        return false;
    }
    let Some(body_start) = code.find('{') else {
        return false;
    };
    matching_brace_end(code, body_start)
        .is_some_and(|end| code[end..].trim().trim_end_matches(';').trim().is_empty())
}

fn is_bare_arrow_expression(code: &str) -> bool {
    let code = strip_trailing_statement_semicolon(code);
    if !code.contains("=>") {
        return false;
    }
    if code.ends_with("()") || code.ends_with(");") {
        return false;
    }
    code.starts_with("async ")
        || code.starts_with('(')
        || code
            .split_once("=>")
            .is_some_and(|(params, _)| is_identifier(params.trim()))
}

fn is_identifier(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn looks_like_returnable_expression(statement: &str) -> bool {
    let statement = statement.trim();
    !statement.is_empty()
        && !matches!(
            statement.split_whitespace().next(),
            Some(
                "const"
                    | "let"
                    | "var"
                    | "return"
                    | "if"
                    | "for"
                    | "while"
                    | "switch"
                    | "try"
                    | "catch"
                    | "function"
                    | "class"
                    | "throw"
                    | "export"
                    | "import"
            )
        )
}
