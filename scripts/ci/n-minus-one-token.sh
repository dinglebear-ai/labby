#!/usr/bin/env bash
# Shared by adapters that use a baseline-generated credential. The adapter
# supplies read_current_token and a baseline_token_path outside upgraded state.

: "${baseline_token_path:?baseline token path required}"

current_token() {
    local token
    # Preserve trailing empty matches so a duplicate empty assignment cannot
    # disappear through command substitution's newline trimming.
    token=$(read_current_token && printf '.') || return
    token=${token%.}
    token=${token%$'\n'}
    [[ -n "$token" && "$token" != *$'\n'* ]] || {
        echo 'baseline credential is missing or ambiguous' >&2; return 1;
    }
    printf '%s\n' "$token"
}

capture_baseline_token() (
    umask 077
    local token
    token=$(current_token) || return
    mkdir -p "$(dirname "$baseline_token_path")"
    # Never replace retained authority during a repeated seed attempt.
    set -o noclobber
    printf '%s\n' "$token" >"$baseline_token_path"
)

retained_token() {
    local baseline current
    baseline=$(cat "$baseline_token_path") || return
    current=$(current_token) || return
    [[ -n "$baseline" && "$baseline" != *$'\n'* && "$current" == "$baseline" ]] || {
        echo 'baseline credential was lost or replaced' >&2; return 1;
    }
    printf '%s\n' "$baseline"
}
