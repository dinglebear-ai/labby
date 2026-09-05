#!/usr/bin/env bash
set -euo pipefail
syft_bin=${SYFT_BIN:-syft}
for archive in lab-*.tar.gz lab-*.zip; do
  [[ -f "$archive" ]] || continue
  output=${archive%.tar.gz}.spdx.json
  output=${output%.zip}.spdx.json
  subject_dir=$(mktemp -d)
  case "$archive" in
    *.tar.gz) tar -xzf "$archive" -C "$subject_dir" ;;
    *.zip) unzip -q "$archive" -d "$subject_dir" ;;
  esac
  "$syft_bin" "dir:$subject_dir" -o "spdx-json=$output"
  rm -rf "$subject_dir"
done
for installer in labby-install.sh labby-install.ps1; do
  [[ -f "$installer" ]] || { echo "missing installer subject: $installer" >&2; exit 1; }
  subject_dir=$(mktemp -d)
  cp "$installer" "$subject_dir/"
  "$syft_bin" "dir:$subject_dir" -o "spdx-json=$installer.spdx.json"
  rm -rf "$subject_dir"
done
[[ -f lab-container-image.tar ]] || { echo 'missing tested container image' >&2; exit 1; }
"$syft_bin" "docker-archive:lab-container-image.tar" -o spdx-json=lab-container-image.spdx.json
