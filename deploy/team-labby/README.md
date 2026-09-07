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
   public path when preserving the existing client URL.
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
`state/labby/.env`, then start the complete stack:

```sh
docker compose --env-file .env up -d
docker compose --env-file .env ps
```

## Publishing launch blocker

Publishing is intentionally fail-closed in this package. Labby currently uses
one service bearer when it calls each Depot. Giving that shared bearer
`skills:write` would make every publish appear as the same service principal;
it would not preserve the authenticated employee's identity or provide
per-user authorization and revocation at Depot.

Do not add `skills:write` to `TEAM_DEPOT_TOKEN` and do not set
`DEPOT_CONTROL_PLANE_SERVICE_WRITES=true`. Team publishing may launch only after
the per-user Labby-to-Depot delegated authorization bridge is implemented and
qualified end to end. Catalog service writes remain disabled permanently.

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
- a publish through Labby fails closed until the delegated authorization bridge
  is implemented;
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
