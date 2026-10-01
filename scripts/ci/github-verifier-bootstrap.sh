# shellcheck shell=sh
# Generated trust code; edit this template and reviewed bootstrap-pins.json.
# Source this helper and call ensure_github_verifier <private temporary directory>.
github_verifier_version_ok() {
    "$1" --version 2>/dev/null | awk '
      NR == 1 && $1 == "gh" && $2 == "version" {
        split($3,v,".");
        if (v[1] ~ /^[0-9]+$/ && v[2] ~ /^[0-9]+$/ && v[3] ~ /^[0-9]+$/ &&
            (v[1] > 2 || (v[1] == 2 && (v[2] > 102 || (v[2] == 102 && v[3] >= 0))))) ok=1
      } END { exit !ok }'
}
ensure_github_verifier() {
    verifier_tmp=$(cd "$1" && pwd) || return 1
    GH_VERIFIER=${GH_VERIFIER:-$(command -v gh 2>/dev/null || true)}
    if [ -n "$GH_VERIFIER" ] && github_verifier_version_ok "$GH_VERIFIER" && "$GH_VERIFIER" attestation verify --help >/dev/null 2>&1; then
        case "$GH_VERIFIER" in /*) ;; *) GH_VERIFIER="$(cd "$(dirname "$GH_VERIFIER")" && pwd)/$(basename "$GH_VERIFIER")" ;; esac
        return 0
    fi
    verifier_platform="$(uname -s)/$(uname -m)"
    case "$verifier_platform" in
        Linux/x86_64) verifier_asset="gh_2.102.0_linux_amd64.tar.gz"; verifier_digest="bb766f710eef8ede859c18578c72c327597cd4c8a85b06001b1f3843c6019386"; verifier_member="gh_2.102.0_linux_amd64/bin/gh" ;;
        Linux/aarch64|Linux/arm64) verifier_asset="gh_2.102.0_linux_arm64.tar.gz"; verifier_digest="7862c86c72f43df3a2d93ddde6f473285b4e2af61b494849846827e513ef6484"; verifier_member="gh_2.102.0_linux_arm64/bin/gh" ;;
        Darwin/arm64) verifier_asset="gh_2.102.0_macOS_arm64.zip"; verifier_digest="da922c20d1792e5b2cbf375593d7a658acf034c12c84e007e71c76ef959c337e"; verifier_member="gh_2.102.0_macOS_arm64/bin/gh" ;;
        MINGW*/x86_64|MSYS*/x86_64|CYGWIN*/x86_64) verifier_asset="gh_2.102.0_windows_amd64.zip"; verifier_digest="ae64e556ecc240b200f7eba60d550e4bb60d78e860e69dd88c449405b86067f4"; verifier_member="bin/gh.exe" ;;
        MINGW*/aarch64|MINGW*/arm64|MSYS*/aarch64|MSYS*/arm64) verifier_asset="gh_2.102.0_windows_arm64.zip"; verifier_digest="5dcf12aa8525eabd0c46ec414f323ab6cf65229fc2cd46543cc705001bbaf223"; verifier_member="bin/gh.exe" ;;
        *) printf '%s\n' 'No pinned GitHub verifier is available for this platform.' >&2; return 1 ;;
    esac
    verifier_archive="$verifier_tmp/$verifier_asset"
    printf '%s\n' 'Bootstrapping pinned GitHub verifier 2.102.0 (no account required).' >&2
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 --max-redirs 5 -fSL --connect-timeout 10 --max-time 120 --max-filesize 25000000 --retry 2 -o "$verifier_archive" "https://github.com/cli/cli/releases/download/v2.102.0/$verifier_asset" || return 1
    if command -v sha256sum >/dev/null 2>&1; then
        verifier_actual=$(sha256sum "$verifier_archive" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        verifier_actual=$(shasum -a 256 "$verifier_archive" | awk '{print $1}')
    else
        printf '%s\n' 'A SHA-256 tool is required for the verifier bootstrap.' >&2; return 1
    fi
    [ "$verifier_actual" = "$verifier_digest" ] || { printf '%s\n' 'Pinned verifier checksum FAILED; downloaded code was not executed.' >&2; return 1; }
    case "$verifier_asset" in
        *.tar.gz) tar -xzf "$verifier_archive" -C "$verifier_tmp" "$verifier_member" || return 1 ;;
        *.zip) command -v unzip >/dev/null 2>&1 || { printf '%s\n' 'unzip is required for the macOS verifier bootstrap.' >&2; return 1; }; unzip -q "$verifier_archive" "$verifier_member" -d "$verifier_tmp" || return 1 ;;
    esac
    GH_VERIFIER="$verifier_tmp/$verifier_member"
    [ -f "$GH_VERIFIER" ] && [ ! -L "$GH_VERIFIER" ] || return 1
    chmod 700 "$GH_VERIFIER" || return 1
    if ! github_verifier_version_ok "$GH_VERIFIER" || ! "$GH_VERIFIER" attestation verify --help >/dev/null 2>&1; then
        printf '%s\n' 'Pinned verifier does not support the required policy.' >&2
        return 1
    fi
}
