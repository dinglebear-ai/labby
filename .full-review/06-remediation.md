# Full Review Remediation Ledger

All 49 findings from `05-final-report.md` are required. Status is updated only after implementation and focused verification.

## Root integration stream

- [x] F-01 current-tree redaction and stale host HMAC removal
- [x] F-01 full current/history validation and revoked-history policy
- [x] F-17 neutral product composition connector and layer test
- [x] Cross-stream integration, generated contracts, all-features verification
- [x] Commit, push, merge, protected no-MCP sync, and branch/worktree cleanup

## Stream A — security, auth, Setup, Doctor, contracts

- [x] F-02
- [x] F-03
- [x] F-04
- [x] F-08
- [x] F-13
- [x] F-14
- [x] F-16
- [x] F-18
- [x] F-19
- [x] F-20
- [x] F-22
- [x] F-34
- [x] F-36
- [x] F-42 Setup/auth portion
- [x] F-44 config/auth persistence portion

## Stream B — gateway, Code Mode, UI, runtime performance

- [x] F-05
- [x] F-06
- [x] F-07
- [x] F-12
- [x] F-15
- [x] F-23
- [x] F-24
- [x] F-25
- [x] F-29
- [x] F-30
- [x] F-31 gateway/Code Mode portion
- [x] F-33
- [x] F-43
- [x] F-45
- [x] F-48
- [x] F-42 runtime-state portion
- [x] F-44 Code Mode/router portion

## Stream C — CI, release, docs, Palette, supply chain

- [x] F-09
- [x] F-10
- [x] F-11
- [x] F-21
- [x] F-26
- [x] F-27
- [x] F-28
- [x] F-32
- [x] F-35
- [x] F-37
- [x] F-38
- [x] F-39
- [x] F-40
- [x] F-41
- [x] F-46
- [x] F-47
- [x] F-49

## Verification evidence

- Authoritative Rust workspace: `just test` — 2,076 passed, 8 skipped.
- Rust quality and supply chain: `just lint`, `just deny`, generated docs checks, CI policy tests, OpenWiki drift check, and actionlint passed.
- Gateway Admin: 340 unit tests, 2 installer tests, TypeScript, production build, five browser scenarios, and compressed route budgets passed.
- Palette: renderer coverage/typecheck/build plus Tauri tests, build, and advisory policy passed.
- Secret response: current documents were redacted, the stale host HMAC value was removed, and full-history scanning is constrained to 11 exact revoked fingerprints. Rewriting published Git history remains intentionally out of scope without separate destructive authorization.
- External operator action: the MCP Registry DNS private key publisher was hardened and documented, but rotating the DNS key and unreadable GitHub secret requires Cloudflare/GitHub secret authority unavailable to this checkout.
