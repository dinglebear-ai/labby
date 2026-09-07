# Team Labby persistent-VM deployment

This package runs the first private tenant of the Labby + Depot platform on one
persistent Linux VM. Labby is the only published service. The Linear
notification worker remains an OAuth upstream, and two temporarily separate
Depot processes isolate the team catalog from the imported catalog.

R2 stores CAS bytes. Docker volumes retain each Depot's mutable indexes,
manifests, sources, jobs, and credentials. The volumes are required even when
every referenced artifact byte already exists in R2.

## Prepare

1. Install Docker Engine and the Compose plugin on an x86_64 Linux VM.
2. Copy this directory to a deployment-owned path such as
   `/srv/team-labby`.
3. Copy `.env.example` to `.env`, `team-depot.env.example` to
   `team-depot.env`, and `catalog-depot.env.example` to `catalog-depot.env`.
4. Create the deployment-owned Labby state directory, then seed its mutable
   configuration and secret file:

   ```sh
   install -d -m 0700 -o 1000 -g 1000 state/labby
   install -m 0600 -o 1000 -g 1000 config.toml.example state/labby/config.toml
   install -m 0600 -o 1000 -g 1000 labby.env.example state/labby/.env
   ```

5. Replace every placeholder. Keep `.env`, `*.env`, and `state/` out of source
   control. Give the team and catalog Depots different R2 credentials scoped to
   different prefixes (or different buckets); neither credential may access
   the other Depot's objects.
6. Set `public_host` to the exact host clients use. Keep `/mcp/linear` as the
   public path when preserving the existing client URL. Set the route target's
   `project_id` to the same bound project configured for Depot delegation.
7. Set `LABBY_AUTH_ALLOWED_EMAIL_DOMAINS` in `state/labby/.env` to the verified
   company domain. If domain-wide admission is not intended, leave it empty and
   add each employee to Labby's persisted allowlist before cutover.

Render the deployment before creating anything:

```sh
docker compose --env-file .env config --quiet
```

## Bootstrap the private Depot credentials

Start only the Depots, wait for both health checks, then mint one read-only
service token inside each persistent data volume:

```sh
docker compose --env-file .env up -d team-depot catalog-depot
docker compose --env-file .env exec team-depot \
  /app/bin/depot rpc 'Depot.Auth.Token.create("team-labby", ["skills:read"]) |> elem(1) |> IO.puts()'
docker compose --env-file .env exec catalog-depot \
  /app/bin/depot rpc 'Depot.Auth.Token.create("team-labby", ["skills:read"]) |> elem(1) |> IO.puts()'
```

Put the resulting values in `TEAM_DEPOT_TOKEN` and `CATALOG_DEPOT_TOKEN` in
`state/labby/.env`. Copy the team read token into `LABBY_DEPOT_TOKEN` there as
well, then start the complete stack:

```sh
docker compose --env-file .env up -d
docker compose --env-file .env ps
```

## Configure delegated team publishing

The service bearer remains read-only. For each write, Labby revalidates the
employee's bound grant and signs a fresh, single-operation assertion lasting no
more than 60 seconds. Depot verifies that assertion and consumes its `jti` once.
Do not add `skills:write` to `TEAM_DEPOT_TOKEN` and do not enable
`DEPOT_CONTROL_PLANE_SERVICE_WRITES`; either change would bypass the delegated
employee authority.

The mappings in `state/labby/.env` and `team-depot.env` are one exact contract:

- `LABBY_PUBLIC_URL` equals `DEPOT_OAUTH_ISSUER`, without authority aliases;
- `LABBY_DEPOT_DELEGATION_AUDIENCE` equals `DEPOT_OAUTH_AUDIENCE`;
- deployment, account, tenant, and optional team IDs equal the corresponding
  `DEPOT_*` identity values;
- Depot's delegation actor equals the durable value in
  `state/labby/installation-id` (`act.sub` in each assertion);
- Depot's organization and project IDs equal the employee grant's bound
  organization and project; and
- the membership-policy epoch equals the current project-policy epoch, shared by
  every member of the bound project, and the organization-policy and
  project-policy epochs equal the current values used when Labby issues that
  bound grant. Labby still revalidates the exact employee membership before it
  mints every assertion.

Labby creates `state/labby/installation-id` at its first start. Read that file
locally, put its exact value in `DEPOT_OAUTH_DELEGATION_ACTOR`, then recreate
`team-depot`. Never derive this value from a hostname or accept it from a
request. Update each configured epoch whenever its authoritative membership or
policy epoch advances; stale assertions must fail closed.

Depot verifies Labby's Ed25519 assertions through
`DEPOT_OAUTH_JWKS_URI`, which must be the public Labby issuer plus `/jwks` and
must be reachable from `team-depot`. Keep the Labby signing key in persistent
`state/labby`; restoring or rotating it requires verifying that the live JWKS
contains the active `kid` before writes are enabled. Catalog Depot has no
delegation verifier and its service writes remain disabled.

Team publishing is a deployment prerequisite, not an automatic consequence of
starting Compose. Enable client traffic only after a non-admin employee's live
bound grant contains the configured organization, project, and current epochs,
Depot can fetch Labby's live JWKS, and the delegated publish/replay/stale-policy
qualification below passes.

An admitted ordinary member needs the `lab` OAuth scope in addition to the
route's baseline `lab:read` scope. On the protected `/mcp/linear` route,
`tools/list` must then include the route-owned `depot_publish` tool. Publishing
uses only the `depot.publish_skill_archive` action and accepts a bounded,
base64-encoded skill archive:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "depot_publish",
    "arguments": {
      "action": "depot.publish_skill_archive",
      "params": {
        "filename": "example-skill.tar.gz",
        "archive_base64": "<base64-encoded-archive>",
        "namespace": "team"
      }
    }
  }
}
```

`namespace` is optional. The tool must remain absent from the root MCP route,
unprotected routes, and routes that do not contain the `team-depot` provider.

On the first authenticated request to a project-bound route containing the
`team-depot` provider, Labby provisions the caller's already-verified external
identity into that exact route project with the fixed `member` role. Provisioning
rechecks the current persisted inbound identity and the current email/domain
admission policy; it is idempotent and never accepts a role, organization, or
project from the request. An identity already bound to another organization, or
a revoked/disabled principal, link, or membership, fails closed and is never
reactivated automatically. Offboarding must disable the membership; also remove
an explicit per-email admission entry when one exists.

## Employee Linear authorization

After an admitted non-admin employee signs in to Labby, that employee must
separately authorize the `linear` upstream from their authenticated session.
Use the Gateway Console authorization action, or its equivalent
`POST /v1/gateway/oauth/start { "upstream": "linear" }` API flow, and complete
the returned authorization URL. Repeat this for every employee; do not use the
bootstrap admin as the per-user qualification account because admin upstream
OAuth intentionally uses Labby's shared gateway subject.

Do not publish either Depot port. The regular private bridge permits required
outbound R2 and notification-worker traffic while keeping the Depots reachable
only by other stack services.

## Edge routing

Point the public host at `127.0.0.1:${LABBY_HOST_PORT:-8765}` using a reverse
proxy or Cloudflare Tunnel. Preserve `Host`, the original scheme through
`X-Forwarded-Proto`, `Authorization`, `Accept`, `Content-Type`,
`Mcp-Protocol-Version`, and all `Mcp-*` headers. Disable request/response
buffering and response compression for `/mcp/linear`, and permit long-lived
SSE connections.

The route-specific protected-resource metadata path must reach Labby too:

```text
/.well-known/oauth-protected-resource/mcp/linear
```

## Required qualification

Before switching client traffic, prove all of the following against immutable
image digests:

- both Depot health endpoints become ready and authenticated discovery reports
  the configured account, distinct tenant, and distinct deployment ids;
- each Depot can read a known R2-backed artifact from its own prefix;
- neither Depot can write with its read-only service token;
- the team Depot accepts one delegated publish for the authenticated employee,
  records that employee as the principal and the Labby installation as actor,
  and rejects replaying the same assertion;
- mismatched issuer, audience, authority IDs, actor, or any stale policy epoch
  rejects the write;
- an employee completes Labby login and the separate Linear upstream login;
- one client URL lists both Depot catalogs and Linear tools;
- stopping and recreating the stack preserves both Depot catalogs and Labby
  identity/OAuth state;
- unauthenticated route responses expose no backend URL or credential detail;
- a backup can be restored into replacement volumes before DNS cutover.

The two Depot processes, separate runtime secret files, and separately scoped
R2 credentials are an initial isolation boundary, not the eventual SaaS tenancy
model. The long-term control plane uses tenant-aware metadata and delegated
authorization while preserving hard user and organization isolation.
