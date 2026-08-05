#!/usr/bin/env bash
#
# Render .github/social-preview.png from social-preview.template.html.
#
# The template sets the wordmark in Manrope 800, which is not installed on the
# fleet. Rather than depend on a system font, the variable font is fetched and
# embedded as a base64 data URI, then the page is rasterised by headless Chrome
# and downsampled from 2x with Lanczos. Nothing about the output depends on the
# viewer having Manrope, which is why this ships as a PNG and not an SVG with
# live text.
#
# Reproducibility: the font is pinned by commit SHA and verified by digest, and
# Pillow is pinned. Chrome is NOT pinned -- it is whatever is on PATH -- so the
# output is byte-identical only for a given Chrome build. The committed PNG was
# rendered with Chrome 151. A different major version may re-rasterise the text
# slightly; that is a visual no-op but will change the file hash.
#
# Uploading the result is a MANUAL step -- see README.md in this directory.
#
# Usage:  docs/assets/brand/render-social-preview.sh
#         OUT=/tmp/preview.png docs/assets/brand/render-social-preview.sh
set -euo pipefail

# Pinned to the commit that last touched this file (2021-08-26). Never track a
# branch here: an unverified 200 response that is not a font (LFS pointer,
# captive portal, redirect to HTML) renders a plausible-looking card in a
# fallback sans and would be committed and uploaded unnoticed.
FONT_COMMIT='8f9a401dbb3793e0d1264b15d96aa253f05280f5'
FONT_URL="https://github.com/google/fonts/raw/${FONT_COMMIT}/ofl/manrope/Manrope%5Bwght%5D.ttf"
FONT_SHA256='d0639be45d0af36e798172419d7bd173c4bd4f29e2b76cbb69db1d11bf8b0a40'
PILLOW_PIN='pillow==12.3.0'

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Prefer git, but do not hard-depend on it: the layout is fixed relative to
# this script, so an export/tarball without .git still works.
ROOT="$(git -C "$DIR" rev-parse --show-toplevel 2>/dev/null || (cd "$DIR/../../.." && pwd))"
TEMPLATE="$DIR/social-preview.template.html"
OUT="${OUT:-$ROOT/.github/social-preview.png}"

CHROME="${CHROME:-}"
if [[ -z "$CHROME" ]]; then
  for c in google-chrome google-chrome-stable chromium chromium-browser chrome; do
    if command -v "$c" >/dev/null 2>&1; then CHROME="$c"; break; fi
  done
fi
[[ -n "$CHROME" ]] || { echo "error: no chrome/chromium on PATH (set CHROME=)" >&2; exit 1; }
command -v curl >/dev/null || { echo "error: curl required" >&2; exit 1; }

# Resolve the Python runner up front: it is needed for font inlining, not just
# the Pillow step, so a host with uv but no system python3 must not die later.
# --no-project keeps uv from resolving whatever pyproject.toml happens to be in
# the caller's CWD.
if python3 -c 'import PIL' >/dev/null 2>&1; then
  PY_RUN=(python3)
elif command -v uv >/dev/null 2>&1; then
  PY_RUN=(uv run --quiet --no-project --with "$PILLOW_PIN" python)
elif command -v python3 >/dev/null 2>&1; then
  echo "error: python3 found but Pillow missing, and uv is not on PATH" >&2; exit 1
else
  echo "error: need python3 with Pillow, or uv on PATH" >&2; exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM

echo "==> fetching Manrope (pinned ${FONT_COMMIT:0:12})"
curl -sSLf -o "$WORK/Manrope.ttf" "$FONT_URL"
echo "$FONT_SHA256  $WORK/Manrope.ttf" | sha256sum -c --status - || {
  echo "error: Manrope digest mismatch -- refusing to render" >&2
  echo "  expected $FONT_SHA256" >&2
  echo "  got      $(sha256sum "$WORK/Manrope.ttf" | cut -d' ' -f1)" >&2
  exit 1
}

echo "==> inlining font into template"
FONT_TTF="$WORK/Manrope.ttf" TEMPLATE="$TEMPLATE" CARD_HTML="$WORK/card.html" \
  "${PY_RUN[@]}" - <<'PY'
import base64, os, pathlib
font = base64.b64encode(pathlib.Path(os.environ['FONT_TTF']).read_bytes()).decode()
html = pathlib.Path(os.environ['TEMPLATE']).read_text(encoding='utf-8')
if '__FONT__' not in html:
    raise SystemExit('error: __FONT__ placeholder missing from template')
pathlib.Path(os.environ['CARD_HTML']).write_text(
    html.replace('__FONT__', font), encoding='utf-8')
PY

# Build a proper file:// URI so a $TMPDIR containing spaces or reserved
# characters cannot produce a URL Chrome silently fails to load.
CARD_URI="$("${PY_RUN[@]}" -c 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).resolve().as_uri())' "$WORK/card.html")"

# Chrome's own sandbox is the containment layer for rasterising a downloaded
# font, so keep it unless we are root (where it cannot start without it).
SANDBOX_FLAG=()
[[ "$(id -u)" -eq 0 ]] && SANDBOX_FLAG=(--no-sandbox)

echo "==> rasterising at 2x with $CHROME"
if ! "$CHROME" \
  --headless=new --disable-gpu --hide-scrollbars "${SANDBOX_FLAG[@]}" \
  --force-device-scale-factor=2 --window-size=1280,640 \
  --user-data-dir="$WORK/profile" \
  --screenshot="$WORK/card@2x.png" \
  "$CARD_URI" >"$WORK/chrome.log" 2>&1; then
  echo "error: chrome exited non-zero:" >&2
  cat "$WORK/chrome.log" >&2
fi
[[ -f "$WORK/card@2x.png" ]] || {
  echo "error: chrome produced no screenshot" >&2
  cat "$WORK/chrome.log" >&2
  exit 1
}

echo "==> downsampling to 1280x640"
SRC="$WORK/card@2x.png" OUT="$OUT" "${PY_RUN[@]}" - <<'PY'
import os
from PIL import Image
src = Image.open(os.environ['SRC']).convert('RGB')
if src.size != (2560, 1280):
    raise SystemExit(f'error: expected a 2560x1280 render, got {src.size[0]}x{src.size[1]}')
out = os.environ['OUT']
tmp = out + '.tmp'
src.resize((1280, 640), Image.LANCZOS).save(tmp, 'PNG', optimize=True)
# Replace atomically: a half-written PNG here would silently corrupt the
# committed asset.
os.replace(tmp, out)
PY

echo "==> wrote $OUT"
echo
echo "REMINDER: committing this file does not change what GitHub serves."
echo "Upload it at Settings > General > Social preview (requires repo admin)."
