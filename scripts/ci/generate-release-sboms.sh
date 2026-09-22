#!/usr/bin/env bash
set -euo pipefail
syft_bin=${SYFT_BIN:-syft}
for archive in lab-*.tar.gz lab-*.zip labby-desktop-*.tar.gz; do
  [[ -f "$archive" ]] || continue
  case "$archive" in
    *.tar.gz) output=${archive%.tar.gz}.spdx.json ;;
    *.zip) output=${archive%.zip}.spdx.json ;;
  esac
  subject_dir=$(mktemp -d)
  case "$archive" in
    *.tar.gz) tar -xzf "$archive" -C "$subject_dir" ;;
    *.zip) unzip -q "$archive" -d "$subject_dir" ;;
  esac
  "$syft_bin" "dir:$subject_dir" -o "spdx-json=$output"
  rm -rf "$subject_dir"
done
for appimage in labby-desktop-*.AppImage; do
  [[ -f "$appimage" ]] || continue
  "$syft_bin" "$appimage" -o "spdx-json=$appimage.spdx.json"
done
installer=labby-install.sh
[[ -f "$installer" ]] || { echo "missing installer subject: $installer" >&2; exit 1; }
subject_dir=$(mktemp -d)
cp "$installer" "$subject_dir/"
"$syft_bin" "dir:$subject_dir" -o "spdx-json=$installer.spdx.json"
rm -rf "$subject_dir"
