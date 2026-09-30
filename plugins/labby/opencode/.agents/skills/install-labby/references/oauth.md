# OAuth

OAuth chooses Google **or** Authelia. Google callback: `https://PUBLIC_ORIGIN/auth/google/callback`; Authelia: `https://PUBLIC_ORIGIN/auth/oidc/callback`. OAuth-only has no static bearer; `both` keeps Labby's generated break-glass bearer. Require the HTTPS public origin/admin email; keep provider secrets out of CLI/chat and follow current provider docs.
