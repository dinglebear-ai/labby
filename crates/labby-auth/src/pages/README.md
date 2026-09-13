# OAuth browser documents

Upstream source: `dinglebear-ai/aurora` commit
`4bd1195cd210634872c032bee5b5ae24eef0e69f` (branch `codex/oauth-document`).

`aurora/` is generated upstream from Aurora's `OAuthDocument` component, which
composes the canonical Aurora Button and Lucide icons. It embeds canonical
Aurora dark/light tokens, component styles, Manrope and Inter fonts. The Rust
renderer substitutes escaped text and validated continuation URLs only.

There are no external asset requests, scripts, event handlers, token values,
or browser storage dependencies. OS color preference selects light/dark;
explicit `html.dark` or `html.light` classes can be used by controlled previews.

## Updating

In the Aurora source checkout run `pnpm oauth:build` and
`pnpm exec playwright test --config playwright.oauth.config.ts`.
Then copy the six generated files in `public/oauth-document/` into `aurora/`.
`provenance.json` records every source file and SHA-256; do not edit generated
HTML or CSS locally. Verify `diff -r <aurora-checkout>/public/oauth-document
crates/labby-auth/src/pages/aurora` is empty, then run
`cargo test -p labby-auth --all-features`.

OAuth validation, redirects, status codes, cookie handling and machine-readable
errors remain owned by their existing handlers. These documents only replace
responses that already rendered human-facing HTML.
