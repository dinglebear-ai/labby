---
title: "Container Runtime Contract"
---

# Container Runtime Contract

The production Compose file is a minimal, local-only gateway baseline. Set
`LABBY_IMAGE` to an immutable `ghcr.io/dinglebear-ai/labby@sha256:...` reference
and `LABBY_CONFIG_DIR` to a reviewed configuration directory. It deliberately
has no fixed container name or host-home mount. The process runs as uid/gid
1000, with a read-only root filesystem, every Linux capability dropped, and
only named state/data volumes writable. The HTTP port binds to loopback.

Optional host integrations are not part of the baseline. Enable the explicit
`integrations` profile only after reviewing and setting `LABBY_INTEGRATIONS_DIR`;
that sidecar has no network and receives only that read-only directory.

```sh
LABBY_IMAGE='ghcr.io/dinglebear-ai/labby@sha256:<verified-64-hex-digest>' \
LABBY_RELEASE_TAG='vMAJOR.MINOR.PATCH' \
LABBY_CONFIG_DIR="$PWD/config/labby" \
scripts/run-compose-prod.sh up -d
```

The supported production launcher always runs
`scripts/ci/validate-container-inputs.sh` before Compose. It rejects tags and
malformed digests before Compose can pull or start anything; do not invoke the
production descriptor directly. It also requires GitHub CLI and verifies the
OCI digest's repository, release-workflow signer, stable source tag, and hosted
runner provenance before Compose can pull or start the image.

All reviewed Docker inputs live in `config/container-supply.conf`. Base images
use exact registry digests, Debian packages resolve only from the dated snapshot,
and Node and uv archives are versioned and digest checked. CI validates the
machine-consumed manifest before passing every value into the build and retains
its canonical identity with release qualification evidence.

The image health helper records only timestamps, counters, delays, and recovery
events in the persistent Labby state volume. On the third failure it asks PID 1
to terminate; Compose is the sole restart owner. Repeated requests back off at
1, 2, then 3 seconds and stop after nine failed probes, leaving the container
unhealthy instead of looping forever. Docker's bounded `json-file` rotation
retains at most five 10 MiB files. Preserve the recovery log, Compose event
stream, and recent bounded logs in the qualification bundle before promotion.
The health request has a two-second total timeout (and one-second connection
timeout), leaving headroom inside Compose's five-second deadline to record a
hung endpoint and invoke the same recovery path as an immediate failure.
The persistent recovery log also rotates at 64 KiB and retains a 32 KiB tail;
deployments may lower those ceilings with `LABBY_HEALTH_LOG_MAX_BYTES` and
`LABBY_HEALTH_LOG_KEEP_BYTES` for constrained storage.

The Incus image definition accepts only HTTPS package sources. Provisioning
parses it with a maintained Serde YAML implementation and rejects duplicate
mapping keys, tags, anchors/aliases, and nesting deeper than 32 levels. The
reviewed versions and artifact digests/integrities are inventoried in
`config/incus/provision-supply.json`; CI rejects drift between that manifest and
the image definition, including a mutated value in every input class. Refresh
the manifest and definition together from upstream release attestations before
an intentional upgrade. This makes an empty-cache build resolve the same named
artifacts instead of a mutable latest channel.

The host-side bootstrap must be downloaded as an immutable release artifact and
verified against its separately obtained digest and provenance/signature before
execution; piping a mutable URL to a shell is unsupported.

Before promotion, run `scripts/ci/qualify-container-operator.sh` against the
actual HTTPS operator route. It fails unless TLS validates against the supplied
CA, unauthenticated work is rejected, the protected-resource root is exact, a
representative configured upstream action succeeds, snippet state survives a
real restart, and the deployment backup observer creates a new backup, proves
that backup contains the durable snippet, and restores it after deletion.
The restart and backup observers must be dedicated executable probes; the
qualifier never evaluates shell command strings. The backup observer implements
`latest`, `create`, `contains BACKUP_ID STATE_NAME`, and
`restore BACKUP_ID` subcommands. Route calls have explicit connection and total
deadlines; each observer invocation is also terminated after 30 seconds by
default. Set `LABBY_QUALIFY_OBSERVER_TIMEOUT_SECONDS` only when a deployment's
documented backup or restart service-level objective requires a different bound.
