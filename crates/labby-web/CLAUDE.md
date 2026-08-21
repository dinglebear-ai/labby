# labby-web Instructions

`labby-web` embeds and serves the statically exported gateway-admin assets. It is a narrow asset/runtime boundary, not a second HTTP application framework.

## Rules

- Keep route/auth/business policy in the Labby product host; this crate resolves embedded assets, content types, cache/header metadata, and SPA/static fallbacks.
- Do not add Axum route composition or operator authorization policy here.
- Asset lookup must be deterministic and safe for untrusted request paths.
- Changes that depend on frontend output must be validated against a fresh `apps/gateway-admin/out` export.

## Verification

```bash
cargo test -p labby-web
cargo clippy -p labby-web --all-targets -- -D warnings
```
