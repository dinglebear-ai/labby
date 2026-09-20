---
title: "Restricted Incus Dev Container Provisioning"
created: "2026-09-13"
updated: "2026-09-13"
---

# Restricted Incus Dev Container Provisioning

This is the review artifact for enabling Labby's production Dev Container
runtime on TOOTIE. None of these commands are part of normal Labby startup.
Creating the project, enabling the HTTPS listener, and adding its client
certificate are an explicit operator provisioning boundary. Apply the plan as
one reviewed deployment change after checking the live host inputs and the
proof steps below.

The observed host inputs on 2026-09-13 are:

- the private Labby bridge is `labbybr0` at `10.99.99.1/24`;
- the managed workload network is `agentbr0` and has the `agent-block-lan` ACL;
- the bounded storage pool is `default`; and
- Labby runs as UID/GID 1000 inside `labby-gateway`.

Recheck those values before applying this plan.

## Host project and profile

Run these commands as the TOOTIE Incus administrator during the reviewed
provisioning change:

```bash
export INCUS_DIR=/mnt/cache/incus/state
INCUS=/usr/local/incus/bin/incus
PROJECT=labby-dev-containers

"$INCUS" config set core.https_address=10.99.99.1:8443

# The current Incus server certificate has a `tootie` DNS SAN. Resolve that
# certificate name to the private bridge address inside the gateway.
"$INCUS" exec labby-gateway -- sh -lc '
  grep -qxF "10.99.99.1 tootie" /etc/hosts ||
    printf "%s\n" "10.99.99.1 tootie" >> /etc/hosts
'

"$INCUS" project create "$PROJECT" \
  --description "Labby owner-scoped Dev Containers" \
  --config features.images=true \
  --config features.networks=false \
  --config features.profiles=true \
  --config features.storage.buckets=false \
  --config features.storage.volumes=false \
  --config restricted=true \
  --config restricted.backups=block \
  --config restricted.containers.interception=block \
  --config restricted.containers.lowlevel=block \
  --config restricted.containers.nesting=block \
  --config restricted.containers.privilege=isolated \
  --config restricted.devices.disk=block \
  --config restricted.devices.nic=managed \
  --config restricted.networks.access=agentbr0 \
  --config restricted.storage-pools.access=default \
  --config limits.containers=8 \
  --config limits.instances=8 \
  --config limits.cpu=16 \
  --config limits.memory=32GiB \
  --config limits.disk=400GiB \
  --config limits.disk.pool.default=400GiB \
  --config limits.processes=8192

for PROFILE in labby-builder-web-v1 labby-runtime-isolated-v1 labby-runtime-web-v1; do
  "$INCUS" profile create "$PROFILE" --project "$PROJECT"
  "$INCUS" profile device add "$PROFILE" root disk path=/ pool=default \
    --project "$PROJECT"
  "$INCUS" profile set "$PROFILE" \
    security.privileged=false \
    security.nesting=false \
    security.idmap.isolated=true \
    boot.autostart=false \
    --project "$PROJECT"
done
for PROFILE in labby-builder-web-v1 labby-runtime-web-v1; do
  "$INCUS" profile device add "$PROFILE" eth0 nic name=eth0 network=agentbr0 \
    --project "$PROJECT"
done
```

`restricted=true` blocks sensitive configuration by default. The explicit
restriction keys document the required posture and make later review easier.
The dedicated builder and web runtime profiles contain only a root disk and
managed NIC. The isolated runtime profile has no NIC. The runtime never selects
the default profile implicitly. Do not copy the existing `agent-jail` profile
because it includes a host workspace disk.

Treat these versioned profiles as immutable after publication. Incus profile
edits propagate to existing instances, so a changed policy requires a new
profile name and reviewed digest. The adapter verifies each configured profile
digest before creation; profile order is part of the launch contract.

`features.images=true` gives the project its own image store. Copy only the
reviewed base fingerprint from the default project's image store, then prove
the resulting fingerprint is identical. Do not resolve or copy a mutable alias:

```bash
APPROVED_IMAGE_FINGERPRINT=85d2c274804145119cb9a1f54fdfc88abb0587242217378113c93e4478a68f67
"$INCUS" image info "$APPROVED_IMAGE_FINGERPRINT" --project default
"$INCUS" image copy "local:${APPROVED_IMAGE_FINGERPRINT}" local: \
  --project default \
  --target-project "$PROJECT"
"$INCUS" image info "${APPROVED_IMAGE_FINGERPRINT}" --project "$PROJECT"
```

## Operator build and launch catalog

After creating the profiles, read each through the project-scoped Incus API and
calculate its content digest as `sha256:` followed by SHA-256 of UTF-8 compact
canonical JSON with exactly `config` and `devices`. Sort object keys recursively,
preserve string values, and do not include description, name, or `used_by`.
Retain the actual response and computed digest as provisioning evidence.

Configure the selected `config.toml` using those exact reviewed digests:

```toml
[dev_containers]
catalog_generation = "labby-dev-containers-20260913-v1"

[[dev_containers.builder_profiles]]
name = "labby-builder-web-v1"
content_digest = "sha256:REPLACE_WITH_REVIEWED_PROFILE_DIGEST"

[[dev_containers.runtime_network_profiles]]
# Explicit network mask 0: no network access.
profiles = [{ name = "labby-runtime-isolated-v1", content_digest = "sha256:REPLACE_WITH_REVIEWED_PROFILE_DIGEST" }]

[[dev_containers.runtime_network_profiles]]
web = true
profiles = [{ name = "labby-runtime-web-v1", content_digest = "sha256:REPLACE_WITH_REVIEWED_PROFILE_DIGEST" }]
```

The placeholders are deliberately invalid until replaced with measured digests.
Builder package access is independent of the final runtime network selection.
This initial project permits isolated and outbound-web workloads. It does not
approve LAN, tailnet, or nested-Docker access. Do not map those flags to the web
profile; doing so would misrepresent the selected policy.

Toolchain, agent, and package installation requires exact operator-owned recipe
keys and bounded literal argument lists under `dev_containers.recipes`.
Unconfigured recipe/version combinations are rejected. Secret environment
entries require an approved opaque reference, source environment name, and
allowed target names under `dev_containers.environment_secrets`. Secret values
are resolved after dotenv loading, are not stored in launch manifests, and are
not injected into image builders.

A successful image publication stores an immutable launch manifest binding its
source revision, output image, catalog generation/digest, ordered runtime
profiles, and environment descriptors. New instances copy that manifest
reference. Rebuilding a template does not change an existing instance's launch
record. A legacy approved base image is build input; it is not a runtime
manifest and cannot silently inherit the project's default profile.

## Project-restricted client certificate

Generate the client key inside `labby-gateway` so the private key never crosses
the container boundary:

```bash
"$INCUS" exec labby-gateway --user 1000 --group 1000 -- sh -lc '
  set -eu
  install -d -m 0700 /home/labby/.labby/incus
  openssl req -x509 -newkey rsa:4096 -sha256 -nodes -days 365 \
    -subj /CN=labby-dev-containers \
    -keyout /home/labby/.labby/incus/client.key \
    -out /home/labby/.labby/incus/client.crt
  chmod 0600 /home/labby/.labby/incus/client.key
  chmod 0644 /home/labby/.labby/incus/client.crt
'

PUBLIC_CERT=$(mktemp /tmp/labby-dev-containers-client.XXXXXX.crt)
"$INCUS" file pull \
  labby-gateway/home/labby/.labby/incus/client.crt "$PUBLIC_CERT"
"$INCUS" config trust add-certificate "$PUBLIC_CERT" \
  --name labby-dev-containers \
  --description "Labby Dev Container runtime" \
  --restricted \
  --projects "$PROJECT"
rm -f "$PUBLIC_CERT"

"$INCUS" file push /mnt/cache/incus/state/server.crt \
  labby-gateway/home/labby/.labby/incus/server.crt \
  --uid 1000 --gid 1000 --mode 0644
```

Configure all five variables in Labby's selected durable dotenv file and
restart only after the project and trust restrictions have been verified:

```env
LABBY_DEV_CONTAINER_INCUS_URL=https://tootie:8443
LABBY_DEV_CONTAINER_INCUS_PROJECT=labby-dev-containers
LABBY_DEV_CONTAINER_INCUS_CLIENT_CERT=/home/labby/.labby/incus/client.crt
LABBY_DEV_CONTAINER_INCUS_CLIENT_KEY=/home/labby/.labby/incus/client.key
LABBY_DEV_CONTAINER_INCUS_SERVER_CERT=/home/labby/.labby/incus/server.crt
```

## Pre-deployment proof

Before restarting Labby, use a temporary Incus client configured with the new
certificate and prove all of these conditions:

1. `tootie` resolves to `10.99.99.1` inside `labby-gateway`, TLS hostname
   validation succeeds against the pinned server certificate, and
   `GET /1.0/instances?project=labby-dev-containers` succeeds.
2. The same certificate cannot list, read, import, publish, or mutate images in
   the `default` project. It can read the single copied fingerprint and publish
   builder output only in `labby-dev-containers`.
3. It cannot create a privileged or nested container, add a host-path disk,
   add an unmanaged NIC, select another storage pool, or target another
   network.
4. A container created from the exact approved fingerprint receives the
   requested instance CPU, memory, process, and root-disk limits.
5. `agent-block-lan` blocks bridge peers, RFC 1918, link-local, and Tailscale
   ranges while allowing the intended outbound package sources.
6. Start, stop, inspect, and delete work only inside the restricted project.

After restart, exercise Labby's `create`, `list`, `stop`, `start`, `reconcile`,
and `destroy` actions through an authenticated product surface. Retain the
ledger transitions and Incus instance labels as evidence. A failed proof keeps
the runtime unconfigured; it is not a reason to grant a host socket, privileged
mode, nesting, or a host-path device.

The restriction model follows the Incus project configuration and confined TLS
client contracts:

- <https://linuxcontainers.org/incus/docs/main/reference/projects/>
- <https://linuxcontainers.org/incus/docs/main/howto/projects_confine/>
- <https://linuxcontainers.org/incus/docs/main/profiles/>
- <https://linuxcontainers.org/incus/docs/main/reference/instance_options/>
