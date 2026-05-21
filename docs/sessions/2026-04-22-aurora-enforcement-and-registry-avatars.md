# Aurora Enforcement, Registry Avatars, and Brand Icon Polish

**Date:** 2026-04-22
**Branch:** `feat/gateway-chat-registry-log-ui`
**Commits pushed:** `0d1acba`, `8233ac5`, `fca019b`

## Session Overview

Three shipped threads:

1. **Aurora design-system enforcement** — applied contract edits to `docs/design-system-contract.md` (Authentication Surfaces, banned-token table, typography rules, Display Slot Assignments); swept 19 product files replacing shadcn-generic tokens (`text-muted-foreground`, `bg-card`, `bg-muted`, `bg-background`, `border-border`, `text-foreground`, `rounded-xl`) with Aurora equivalents; added an ESLint `no-restricted-syntax` rule to prevent regressions.
2. **Rust test-fixture repair** — fixed pre-existing E0063 compile errors on the feature branch by adding `proxy_prompts` to `UpstreamConfig` struct literals across 4 files and `search` to the `StoreListParams` literal in `api/services/registry_v01.rs`.
3. **Registry GitHub avatars + brand icon polish** — new `githubAvatarFromRepoUrl(repoUrl)` helper derives `https://github.com/<owner>.png` from `server.repository.url`, used as the primary image in the registry list rows and detail header; gateway-form brand chip repainted with white background, colored border, and SVG fill recoloring for stronger contrast on pale palettes.

Version bumped `0.7.1 → 0.7.2` (workspace) and `0.2.1 → 0.2.2` (gateway-admin); CHANGELOG released `[0.7.1]` and opened `[Unreleased] — 0.7.2` with a commit table for the five feat commits that landed on this branch.

## Timeline

1. Completed in-progress codebase sweep applying the banned-token table to 19 product files; verified `tsc --noEmit` exit 0 and `auth-bootstrap.test.tsx` still passes.
2. Migrated 12 `rounded-xl` occurrences in product code to `rounded-aurora-2`.
3. Committed sweep + contract + eslint rule as `0d1acba`; pushed with `-u origin feat/gateway-chat-registry-log-ui`.
4. Investigated user-flagged pre-existing Rust breakage; located 11 E0063 errors via `cargo check --all-features --tests`; added `proxy_prompts: <val>` after each `proxy_resources:` line in `UpstreamConfig` struct literals across `gateway/config.rs`, `upstream/pool.rs`, `tests/upstream_oauth.rs`, and `oauth/upstream/cache.rs`; added missing `search: None` to `StoreListParams` literal in `registry_v01.rs`.
5. Authored the ESLint rule targeting `JSXAttribute[name.name='className'] Literal|TemplateElement` with a banned-token regex; scoped it to `app/**` + `components/**` and exempted `components/ui/**`.
6. Implemented `lib/github-avatar.ts` helper; wired it into `registry-list-content.tsx` and `server-detail-panel.tsx` as the primary image source with `icons[0]` and `Package` lucide icon fallbacks; committed as `8233ac5`.
7. Final quick-push: received a local edit to `gateway-form-dialog.tsx` (brand chip repaint); bumped versions; rolled the Unreleased section into `[0.7.1]`, opened `[Unreleased] — 0.7.2` with a commit table; committed as `fca019b` and pushed.

## Key Findings

- `apps/gateway-admin/eslint.config.mjs` uses the flat-config `tseslint.config()` form, so per-file-glob rule blocks must follow the typed block. The new banned-token rule uses `files: ['app/**/*.{ts,tsx}', 'components/**/*.{ts,tsx}']` followed by an explicit `components/ui/**` override with `'no-restricted-syntax': 'off'`.
- `lib/github-avatar.ts:1-23` — `https://github.com/<owner>.png?size=120` is the GitHub avatar endpoint that transparently resolves for users and organizations alike, so a single URL shape handles both cases without an API call.
- `components/registry/registry-list-content.tsx:218` and `components/registry/server-detail-panel.tsx:103` both derive an avatar URL with `githubAvatarFromRepoUrl(server.repository?.url) ?? safeHref(primaryIcon?.src) ?? null` and render a `Package` lucide icon when both sources are absent or the image fails to load.
- `crates/lab/src/config.rs:164` — the `UpstreamConfig` struct grew a `proxy_prompts: bool` field before this branch, but 11 test-fixture struct literals across 4 files were never updated, producing E0063 errors that blocked `cargo check --all-features --tests`. Inserting `proxy_prompts: <same as proxy_resources>` after each `proxy_resources:` line resolved all of them.
- `crates/lab/src/api/services/registry_v01.rs:57-66` — `StoreListParams` literal was missing `search: None`; setting it to `None` and then letting the existing `with_search` path assign it preserved the original intent.

## Technical Decisions

- **ESLint coverage via AST selectors over string matching:** used `no-restricted-syntax` with `JSXAttribute[name.name='className'] Literal[value=/pattern/]` plus a sibling selector for `TemplateElement` instead of adding a regex plugin. This is native to `@typescript-eslint/parser` and catches both static strings and template literals in `cn(...)` calls without needing `eslint-plugin-tailwindcss`.
- **Exempted `components/ui/**` rather than all primitives globally:** the contract explicitly names `components/ui/` as the sanctioned escape hatch where shadcn-generic tokens are permitted; any future shadcn primitives fork into this directory will inherit the exemption automatically.
- **Derived GitHub avatar URL client-side, no API call:** `github.com/<owner>.png` is a stable redirect that returns the same image the GitHub UI shows; avoids rate limits and auth. Added `referrerPolicy="no-referrer"` to prevent leaking internal referrer headers.
- **Fallback chain `ghAvatar → icons[0] → Package icon`:** preserves the previous behavior for non-GitHub repos while upgrading the common case. `onError` swaps to the `Package` lucide icon if the avatar 404s or is blocked, avoiding a broken-image glyph.
- **Bump rule applied as `fix` → patch despite feat commits earlier in the branch:** the visible commit (`fca019b`) is a fix; the feat work from earlier in the branch (`0d1acba`, `8233ac5`) was pushed previously without a version bump. Rolling them into the `[Unreleased] — 0.7.2` changelog section preserves them in the release notes without re-bumping.

## Files Modified

| File | Purpose |
|------|---------|
| `docs/design-system-contract.md` | Added Authentication Surfaces section, banned-shadcn-token table, typography-ramp override rule, Display Slot Assignments table |
| `apps/gateway-admin/eslint.config.mjs` | Added `no-restricted-syntax` rule banning shadcn-generic tokens in className literals + template elements, scoped to `app/**` and `components/**`, with `components/ui/**` exempt |
| `apps/gateway-admin/lib/github-avatar.ts` | New helper `githubAvatarFromRepoUrl(repoUrl)` deriving `github.com/<owner>.png?size=120` |
| `apps/gateway-admin/components/registry/registry-list-content.tsx` | Wired GitHub avatar with `icons[0]` → `Package` fallback chain; added `overflow-hidden` + `size-full object-cover` for full-bleed avatar |
| `apps/gateway-admin/components/registry/server-detail-panel.tsx` | Same avatar wiring in dialog header |
| `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx` | Brand chip repainted: white bg, 2px colored border + colored ring, SVG fill recoloring via `.replace('fill="white"', fill=brand)`, letter fallback inherits brand color |
| 16 files in `components/{gateway,logs,auth,upstream-oauth,setup}/**` + `components/app-*.tsx` + `app/layout.tsx` | Sweep: shadcn-generic tokens replaced with Aurora equivalents; `rounded-xl → rounded-aurora-2` in 6 files |
| `crates/lab/src/dispatch/gateway/config.rs` | Added `proxy_prompts: <val>` to 9 `UpstreamConfig` literals in tests |
| `crates/lab/src/dispatch/upstream/pool.rs` | Added `proxy_prompts: <val>` to 7 `UpstreamConfig` literals in tests |
| `crates/lab/tests/upstream_oauth.rs` | Added `proxy_prompts: <val>` to the `upstream_cfg` helper |
| `crates/lab/src/oauth/upstream/cache.rs` | Added `proxy_prompts: false` to the `cfg(...)` helper |
| `crates/lab/src/api/services/registry_v01.rs` | Added `search: None` to the `StoreListParams` literal |
| `Cargo.toml` | Workspace version `0.7.1 → 0.7.2` |
| `Cargo.lock` | Lock-file refresh from `cargo check` |
| `apps/gateway-admin/package.json` | Version `0.2.1 → 0.2.2` |
| `CHANGELOG.md` | Released `[0.7.1]` with `52ef7d4` commit row; opened `[Unreleased] — 0.7.2` with 5-commit table and Highlights |

## Commands Executed

| Command | Outcome |
|---------|---------|
| `cargo check --all-features --tests` | Initial run: 11 × E0063 on `UpstreamConfig.proxy_prompts` + 2 × E0063 on `StoreListParams.search`. Final run: clean, warnings only. |
| `pnpm exec tsc --noEmit` | Exit 0 after every sweep batch. |
| `pnpm exec tsx --test components/auth/auth-bootstrap.test.tsx` | 1 pass, 0 fail. |
| `pnpm exec eslint 'components/**/*.{ts,tsx}' 'app/**/*.{ts,tsx}'` | 0 banned-token violations; 5 pre-existing unrelated errors (Wrench unused, missing `react-hooks` plugin, no-useless-escape). |
| `git push -u origin feat/gateway-chat-registry-log-ui` | First push set upstream; subsequent pushes plain `git push origin <branch>`. |

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| Registry list row / detail header image | Generic `Package` lucide icon for most servers (few provided explicit `icons[0]`) | GitHub owner avatar for any repo hosted on github.com; falls back to `icons[0]`, then `Package` on error |
| Product-code `className` values | Mix of shadcn-generic tokens (`text-muted-foreground`, `bg-card`, `bg-muted`, etc.) and Aurora tokens | Aurora tokens only; `components/ui/` primitives untouched |
| ESLint | No design-system enforcement | Banned-token rule errors on shadcn-generic tokens in product code |
| `cargo check --all-features --tests` | Failed with 13 × E0063 | Clean |
| Gateway form brand chip | Brand-colored flood fill (low contrast on pale palettes) | White background with colored border + 1px colored ring; SVG glyphs fill-recolored to brand |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --all-features --tests` | 0 errors | 0 errors, warnings only | ✅ |
| `pnpm exec tsc --noEmit` (post-sweep) | exit 0 | exit 0 | ✅ |
| `pnpm exec tsc --noEmit` (post-avatar) | exit 0 | exit 0 | ✅ |
| `pnpm exec tsx --test auth-bootstrap.test.tsx` | 1 pass | 1 pass, 0 fail | ✅ |
| `pnpm exec eslint components/** app/**` | 0 banned-token errors | 0 banned-token errors | ✅ |
| `git log origin/feat/gateway-chat-registry-log-ui..HEAD` after each push | empty | empty (all commits pushed) | ✅ |

## Source IDs + Collections Touched

None. No embed/retrieve operations or vector-store reads occurred in this session.

## Risks and Rollback

- **ESLint rule:** low-risk; can be reverted by removing the two file-scoped rule blocks at the bottom of `eslint.config.mjs`. No transitive effects on runtime code.
- **Token sweep:** mechanical class-name substitution; no logic changes. Rollback via `git revert 0d1acba`. If any visual regression surfaces, inspect individual file diffs — particularly the `bg-muted/NN → bg-aurora-control-surface/NN` cases where opacity interpreted with different underlying color can shift contrast.
- **Rust test-fixture repair:** adding `proxy_prompts` and `search` to struct literals; values chosen to mirror sibling fields (proxy_prompts matches proxy_resources; search defaults to `None` since later code assigns it via `with_search`). No risk to production behavior — test code only.
- **Avatar helper:** `https://github.com/<owner>.png` is an external dependency. If GitHub changes that endpoint, `onError` falls back to `Package`; no crash. `encodeURIComponent(owner)` prevents URL injection from malformed repo URLs.
- **Version bump commit bundles the brand-chip fix with the changelog/version edits:** acceptable for this branch since the branch hasn't been merged yet; if clean history is needed, these could be split before merge.

## Decisions Not Taken

- **Installing `eslint-plugin-tailwindcss`:** rejected — adds a heavyweight dependency, and `no-restricted-syntax` + regex is sufficient for the 6 banned tokens. Reconsider if the banned-token list grows past ~15 or if we need position-aware class-order enforcement.
- **GitHub API call for avatar:** rejected — the unauthenticated `github.com/<owner>.png` redirect returns the same image and doesn't consume the 60 req/hr unauthenticated rate limit.
- **Opus-level Rust refactor to unify `UpstreamConfig` test fixtures behind a builder:** out of scope for "fix the pre-existing breakage"; flagged as a follow-up if the struct grows further.
- **Amending `0d1acba` to include the ESLint rule:** chose to include it in `0d1acba` directly (single commit) rather than splitting the sweep and the rule. Preserves the invariant "sweep + enforcement shipped together."
- **Introducing a `GitHubAvatar` React component:** rejected as premature abstraction — the avatar JSX is 8 lines and duplicated in exactly two call sites; a helper (`githubAvatarFromRepoUrl`) is the right size.

## Open Questions

- The 5 pre-existing lint errors surfaced by `eslint components/** app/**` (Wrench unused in `gateway-detail-content.tsx`, missing `react-hooks/exhaustive-deps` plugin in `gateway-form-dialog.tsx`, no-useless-escape in `server-detail-panel.tsx`) are unrelated to this session's changes but should be addressed in a follow-up pass. The `react-hooks` plugin in particular is referenced without being installed.
- `eslint.config.mjs` doesn't currently use `flat config`'s `languageOptions.parserOptions.projectService` for the new rule blocks — same as the existing setup. No observed issue, but if JSX parsing drifts, the banned-token selector may not fire.
- The `github-avatar.ts` helper does not verify that the repo exists (only that the URL parses as a github.com URL). For repos whose owner was deleted, the avatar URL will 404 and the `onError` fallback will kick in — visible as a brief broken-image flash in some browsers. Unverified.

## Next Steps

- Address the 5 pre-existing ESLint errors in a follow-up (likely `chore(gateway-admin): eslint cleanup`).
- Consider broadening the banned-token ESLint rule to also flag raw hex/`rgba()`/`hsl()` in `className` (contract forbids these; not currently enforced).
- Observe whether the GitHub avatar creates noticeable latency on registry list pages; if so, add a `fetchPriority="low"` and/or intersection-observer-based lazy load.
- Merge or rebase `feat/gateway-chat-registry-log-ui` into main when the `lab-h5pm` RegistryStore work stabilizes.
