# Brand Assets

Rendered brand assets for `labby`, plus the source needed to regenerate them.

| File | What | Consumed by |
|---|---|---|
| `labby-banner.png` | 880x180 README header banner | root `README.md`, and Community Applications via `unraid/ca/labby.xml` |
| `social-preview.png` (in `.github/`) | 1280x640 GitHub social preview card | uploaded by hand — see below |
| `social-preview.template.html` | Source for the card above | `render-social-preview.sh` |
| `render-social-preview.sh` | Renders the card to `.github/social-preview.png` | run by hand |

The 460x460 avatar mark lives at `icon.svg` in the repository root, where the
Unraid plugin and packaging metadata expect it.

## The mark

These assets use the dinglebear-ai **4A Cell Matrix** mark system: a 3x3 dot
field on a 64-unit box, cells at 14 / 32 / 50 on both axes. A lit cell is
r=6.4 in cyan `#29b6f6`, a dim cell is r=3.4 in `#24536c`, and exactly one
role cell per mark is r=6.4 in the product hue. Ground is `#07131c`.

Labby's registered pattern is lit cells (col,row) `0,0 · 0,1 · 0,2 · 1,1 · 2,1`
with the role cell at `2,1` in rose `#f9a8c4`. Wordmark is Manrope 800 at
`-0.042em`, base ink `#e8f4fb` with a cyan `#29b6f6` suffix — `lab` + `by`.

**Do not improvise new marks here.** The system of record — the pattern
register for every repo, plus lockups, badges, mono variants, favicons, and
avatars — is the Claude Design project *"Axon, Labby, Cortex Logo System"*
(`brand/README.md`). Pull from there, and register any new pattern there
first so two repos never collide on the same cell arrangement.

## Why these are PNGs and not SVGs

The lockups and badges set live Manrope 800 text with tuned letter-spacing.
Manrope is not installed on the fleet, so an SVG with live text falls back to a
generic sans and the metrics drift — wrong weight, wrong tracking, misaligned
against the mark. Every asset here is rendered at 2x with the font embedded, so
it carries no font dependency for any viewer.

## Regenerating the social preview

```bash
docs/assets/brand/render-social-preview.sh
```

The script fetches the Manrope variable font, inlines it as a base64 data URI,
rasterises the template with headless Chrome at 2x, and downsamples to exactly
1280x640. It needs Chrome (or Chromium) and `curl`, plus either a Python with
Pillow or `uv` on `PATH`. The font is fetched to a temporary directory and is
never committed.

Set `OUT=` to render somewhere other than `.github/social-preview.png`, and
`CHROME=` to pick a specific browser binary.

Edit `social-preview.template.html` to change the card; the `__FONT__` token in
its `@font-face` block is the substitution point and must survive edits.

### What is pinned, and what "reproducible" means here

The font is pinned to a `google/fonts` **commit SHA** and its SHA-256 is
verified before use; a mismatch aborts without writing anything. This is not
ceremony. `curl -fL` only rejects 4xx/5xx, so any 200 response that is not a
font — an LFS pointer, a captive portal, a redirect to HTML — would otherwise
be embedded and rendered in a fallback sans, producing a card that looks
plausible, passes every check in the script, and gets committed and uploaded
unnoticed. That is precisely the failure this whole PNG-not-SVG approach exists
to prevent.

Pillow is pinned too. **Chrome is not** — it is whatever is on `PATH`. So the
output is byte-identical only for a given Chrome build; the committed PNG was
rendered with Chrome 151. A different major version may re-rasterise the text
slightly, which is a visual no-op but changes the file hash. Do not treat a
hash change after a browser upgrade as a regression.

## Uploading the social preview — this part is manual

**Committing `.github/social-preview.png` does not change what GitHub serves.**
Nothing in CI reads that file. GitHub serves the social preview from an image
uploaded through the web UI, and there is no API or workflow that can set it.

After regenerating, upload it at **Settings → General → Social preview**, which
requires repository admin. Until then, link unfurls keep showing whatever was
uploaded last — potentially an image with long-outdated branding.

The file is kept in the repository anyway so the current card is versioned,
reviewable, and regenerable, rather than existing only inside GitHub's settings.
