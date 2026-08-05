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
# Uploading the result is a MANUAL step -- see README.md in this directory.
#
# Usage:  docs/assets/brand/render-social-preview.sh
set -euo pipefail

FONT_URL='https://github.com/google/fonts/raw/main/ofl/manrope/Manrope%5Bwght%5D.ttf'
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git -C "$DIR" rev-parse --show-toplevel)"
TEMPLATE="$DIR/social-preview.template.html"
OUT="$ROOT/.github/social-preview.png"

CHROME="${CHROME:-}"
if [[ -z "$CHROME" ]]; then
  for c in google-chrome chromium chromium-browser; do
    if command -v "$c" >/dev/null 2>&1; then CHROME="$c"; break; fi
  done
fi
[[ -n "$CHROME" ]] || { echo "error: no chrome/chromium on PATH (set CHROME=)" >&2; exit 1; }
command -v curl >/dev/null || { echo "error: curl required" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> fetching Manrope"
curl -sSLf -o "$WORK/Manrope.ttf" "$FONT_URL"

echo "==> inlining font into template"
FONT_TTF="$WORK/Manrope.ttf" TEMPLATE="$TEMPLATE" CARD_HTML="$WORK/card.html" python3 - <<'PY'
import base64, os, pathlib
font = base64.b64encode(pathlib.Path(os.environ['FONT_TTF']).read_bytes()).decode()
html = pathlib.Path(os.environ['TEMPLATE']).read_text()
if '__FONT__' not in html:
    raise SystemExit('error: __FONT__ placeholder missing from template')
pathlib.Path(os.environ['CARD_HTML']).write_text(html.replace('__FONT__', font))
PY

echo "==> rasterising at 2x with $CHROME"
"$CHROME" \
  --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
  --force-device-scale-factor=2 --window-size=1280,640 \
  --user-data-dir="$WORK/profile" \
  --screenshot="$WORK/card@2x.png" \
  "file://$WORK/card.html" >/dev/null 2>&1 || true
[[ -f "$WORK/card@2x.png" ]] || { echo "error: chrome produced no screenshot" >&2; exit 1; }

# Pillow is not a repo dependency; prefer a local import, else borrow it via uv.
if python3 -c 'import PIL' >/dev/null 2>&1; then
  PY_RUN=(python3)
elif command -v uv >/dev/null 2>&1; then
  PY_RUN=(uv run --quiet --with pillow python)
else
  echo "error: need python3 with Pillow, or uv on PATH" >&2; exit 1
fi

echo "==> downsampling to 1280x640"
SRC="$WORK/card@2x.png" OUT="$OUT" "${PY_RUN[@]}" - <<'PY'
import os
from PIL import Image
src = Image.open(os.environ['SRC']).convert('RGB')
if src.size != (2560, 1280):
    raise SystemExit(f'error: expected a 2560x1280 render, got {src.size[0]}x{src.size[1]}')
src.resize((1280, 640), Image.LANCZOS).save(os.environ['OUT'], 'PNG', optimize=True)
PY

echo "==> wrote $OUT"
echo
echo "REMINDER: committing this file does not change what GitHub serves."
echo "Upload it at Settings > General > Social preview (requires repo admin)."
