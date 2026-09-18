# OAuth setup reference

Use current Labby runtime docs plus current official identity-provider documentation. Search the official docs again if console labels or provider requirements differ from this reference.

## Labby authentication model

Supported server topologies:

| Topology | Labby setup selector | Static bearer |
| --- | --- | --- |
| Bearer only | --auth bearer --oauth none | generated and required |
| OAuth only | --auth oauth --oauth google OR authelia | intentionally absent |
| OAuth + bearer | --auth both --oauth google OR authelia | generated break-glass credential |

For backwards compatibility, --oauth google or --oauth authelia without --auth retains the historical OAuth plus static bearer behavior.

Labby currently selects exactly one inbound OAuth provider per instance. Google plus Authelia simultaneously is not supported.

OAuth mode requires LABBY_AUTH_ADMIN_EMAIL.

## Google OAuth

Current official starting point:
https://console.developers.google.com/apis/credentials/oauthclient

Current official documentation:
https://developers.google.com/workspace/guides/create-credentials

Attempt a real browser handoff when the active agent runtime exposes one. Discover and prefer an already connected Chrome DevTools/browser MCP, Computer Use, Claude-with-Chrome/browser tooling, or ChatGPT/Work cloud browser. Open the current Google Auth Platform Clients page directly. If no usable browser tool exists, give the user the exact URL instead.

Before asking for credentials, compute and display the exact callback:

https://PUBLIC_LABBY_ORIGIN/auth/google/callback

Directions:

1. Select or create the Google Cloud project that will own the Labby OAuth client.
2. Open Google Auth Platform.
3. Configure Branding if the project has not done so:
   - app name: Labby, or the operator's preferred name
   - user support email: operator-selected account
   - contact email: operator-selected account
4. Configure Audience:
   - Internal for a Workspace-only deployment when appropriate
   - External for personal/mixed Google accounts
   - if External remains in testing, add the intended admin account as a test user
5. Open Clients and choose Create Client.
6. Application type: Web application.
7. Name: Labby, or a deployment-specific name.
8. Under Authorized redirect URIs, add exactly the callback shown above.
   - no wildcard
   - no extra path
   - no query string
   - no accidental trailing slash
9. Create the client.
10. Copy the Client ID and Client Secret when the console provides them. If the current console only exposes the secret via a downloaded credential JSON or a newly created client secret, follow the current official UI and keep the secret private.
11. Give the values only to the protected Labby setup prompt/environment.
12. Use openid, email, and profile scopes.

Do not add a JavaScript origin unless the current Labby flow or current official docs require it.

After setup, verify:
- LABBY_AUTH_MODE resolves to oauth
- LABBY_AUTH_PROVIDER resolves to google
- LABBY_AUTH_ADMIN_EMAIL matches the intended bootstrap admin
- public URL and callback match the browser-visible deployment
- bearer credential presence matches oauth-only versus both

Never print the secret or bearer token.

## Authelia OIDC

Callback:

https://PUBLIC_LABBY_ORIGIN/auth/oidc/callback

Labby's current Authelia contract expects:
- exact HTTPS issuer URL
- confidential OIDC client
- client_secret_basic
- authorization code flow
- PKCE S256
- openid, profile, email scopes
- email and email_verified claims
- exact redirect URI above

Collect:
- issuer URL
- client ID
- client secret
- Labby bootstrap admin email

For a private Authelia issuer, inspect current Labby runtime documentation for the trusted-private-origin and custom CA controls. Do not bypass TLS verification.

Before changing an Authelia server configuration, search the current official Authelia docs, back up affected config, show the proposed changes, and obtain explicit approval.
