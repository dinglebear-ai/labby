#!/usr/bin/env bash
set -euo pipefail

inventory=scripts/ci/lifecycle-scripts.json
[[ -f "$inventory" ]] || { echo "missing lifecycle inventory: $inventory" >&2; exit 1; }
shell_files=()
public_copies=()
test_files=()
powershell_files=()
while IFS= read -r path; do shell_files+=("$path"); done < <(python3 -c 'import json; print(*json.load(open("scripts/ci/lifecycle-scripts.json"))["shell"], sep="\n")')
while IFS= read -r path; do public_copies+=("$path"); done < <(python3 -c 'import json; print(*json.load(open("scripts/ci/lifecycle-scripts.json"))["public_copies"], sep="\n")')
while IFS= read -r path; do test_files+=("$path"); done < <(python3 -c 'import json; print(*json.load(open("scripts/ci/lifecycle-scripts.json"))["tests"], sep="\n")')
while IFS= read -r path; do powershell_files+=("$path"); done < <(python3 -c 'import json; print(*json.load(open("scripts/ci/lifecycle-scripts.json"))["powershell"], sep="\n")')

# Fail closed when a shipped shell entrypoint is added without an explicit
# interpreter/analyzer decision in the inventory.
while IFS= read -r path; do
  [[ " ${shell_files[*]} ${test_files[*]} " == *" $path "* ]] || {
    echo "untracked lifecycle shell entrypoint: $path" >&2
    exit 1
  }
done < <(python3 - <<'PY'
from pathlib import Path
roots = [Path("scripts"), Path("plugins/scripts"), Path("unraid/source"), Path("apps/palette-tauri/scripts")]
paths = [Path("install.sh")]
for root in roots:
    paths.extend(path for path in root.rglob("*") if path.is_file())
for path in sorted(set(paths)):
    try:
        header = path.open(errors="ignore").readline()
    except OSError:
        continue
    if header.startswith("#!") and ("sh" in header or "bash" in header):
        print(path)
PY
)

# PowerShell has no required shebang. Discover every shipped .ps1 independently
# so omission from the pinned PSScriptAnalyzer inventory fails closed.
while IFS= read -r path; do
  [[ " ${powershell_files[*]} ${test_files[*]} " == *" $path "* ]] || {
    echo "untracked lifecycle PowerShell entrypoint: $path" >&2
    exit 1
  }
done < <(find scripts plugins/scripts unraid/source apps/palette-tauri/scripts -type f -name '*.ps1' -print | sort)

for path in "${shell_files[@]}" "${powershell_files[@]}" "${test_files[@]}"; do
  [[ -f "$path" ]] || { echo "missing inventoried lifecycle file: $path" >&2; exit 1; }
done
for path in "${shell_files[@]}"; do
  first=$(head -n 1 "$path")
  case "$first" in
    *bash*) bash -n "$path" ;;
    *) sh -n "$path" ;;
  esac
done
shellcheck --severity=warning "${shell_files[@]}"
for copy in "${public_copies[@]}"; do
  cmp -s scripts/install.sh "$copy" || { echo "$copy is not synchronized with scripts/install.sh" >&2; exit 1; }
done
for path in "${test_files[@]}"; do
  case "$path" in
    *.sh) "$path" ;;
  esac
done
