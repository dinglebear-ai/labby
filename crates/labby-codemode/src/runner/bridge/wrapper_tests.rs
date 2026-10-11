use super::*;

#[test]
fn generated_wrapper_uses_the_shared_composed_snippet_budget() {
    let wrapped = wrap_code_mode("async () => ({ ok: true })", "");
    assert!(wrapped.contains(&format!(
        "globalThis.__labSnippetMaxBytes = {MAX_SNIPPET_RESOLVED_BYTES_PER_RUN};"
    )));
}
