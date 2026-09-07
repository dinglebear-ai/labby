# Team Labby persistent-VM deployment

This package runs the first private tenant of the Labby + Depot platform on one
persistent Linux VM. Labby is the only published service. The Linear
notification worker remains an OAuth upstream, and two temporarily separate
Depot processes isolate the writable team catalog from the imported catalog.

R2 stores CAS bytes. Docker volumes retain each Depot's mutable indexes,
manifests, sources, jobs, and credentials. The volumes are required even when
every referenced artifact byte already exists in R2.

## Prepare

1. Install Docker Engine and the Compose plugin on an x86_64 Linux VM.
2. Copy this directory to a deployment-owned path such as
   `/srv/team-labby`.
3. Copy `.env.example` to `.env` and `config.toml.example` to `config.toml`.
4. Replace every placeholder. Keep `.env` mode `0600` and out of source
   control.
5. Set `public_host` to the exact host clients use. Keep `/mcp/linear` as the
   public path when preserving the existing client URL.

Render the deployment before creating anything:

```sh
docker compose --env-file .env config --quiet
```

## Bootstrap the private Depot credentials

Start only the Depots, wait for both health checks, then mint one service token
inside each persistent data volume:

```sh
docker compose --env-file .env up -d team-depot catalog-depot
docker compose --env-file .env exec team-depot \
  /app/bin/depot rpc 'Depot.Auth.Token.create("team-labby", ["skills:read", "skills:write"]) |> elem(1) |> IO.puts()'
docker compose --env-file .env exec catalog-depot \
  /app/bin/depot rpc 'Depot.Auth.Token.create("team-labby", ["skills:read"]) |> elem(1) |> IO.puts()'
```

Put the resulting values in `TEAM_DEPOT_TOKEN` and `CATALOG_DEPOT_TOKEN`, then
start the complete stack:

```sh
docker compose --env-file .env up -d
docker compose --env-file .env ps
```

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

- both Depot health endpoints become ready and report distinct deployment ids;
- each Depot can read a known R2-backed artifact from its own prefix;
- the catalog Depot cannot write with its read-only service token;
- the team Depot accepts a publish through the authenticated Labby route;
- an employee completes Labby login and the separate Linear upstream login;
- one client URL lists both Depot catalogs and Linear tools;
- stopping and recreating the stack preserves both Depot catalogs and Labby
  identity/OAuth state;
- unauthenticated route responses expose no backend URL or credential detail;
- a backup can be restored into replacement volumes before DNS cutover.

The two Depot processes are an initial isolation boundary, not the eventual
SaaS tenancy model. The long-term control plane uses tenant-aware metadata and
authorization while preserving hard user and organization isolation.
