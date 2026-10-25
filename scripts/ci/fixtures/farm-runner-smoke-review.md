# Farm runner acceptance fixture

This file is inactive under `scripts/ci/fixtures/`. No workflow is installed or
published. Labby's `.github/AGENTS.md` and CI contract require GitHub-hosted
product CI. Activating this manual acceptance workflow needs a reviewed explicit
exception and authorized push; it must not reroute existing jobs or modify
`AGENTS.md`.

The intended target is `dinglebear-ai/labby`, chosen by the user for one Farm
runner smoke. Use a unique `farm-u8-smoke-<nonce>` runner label, one owned runner,
2 CPUs and 12 GiB. Register through Farm's existing protected credential path.
Inspect the actual owned container image, cgroup namespace, and mounts before
dispatch; require its Docker socket to be private, not a host socket. No token
is part of this fixture, a workflow input, or a log.

The workflow has manual dispatch only, no checkout, no external action, no
repository/environment secrets, no image pull, and no source-code execution.
Its one job checks actual runner OS/architecture, cgroup limits and Docker
readiness. Job execution is capped at five minutes and its only step at one
minute. These are execution limits; they do not bound GitHub queue wait.

The operator budget is one dispatch and one runner, ten minutes total including
queue wait. Record the dispatch run ID and Farm operation/container/registration
IDs before observation. Cancel only that exact run after the observation budget;
inspect terminal state before any retry. Stop and unregister only the owned
runner via the existing Farm owner, then prove absence and fenced grant release.
Preserve unrelated runners, jobs, caches and GitHub state.

This acceptance does not replace Labby CI, the complete Core local-check suite,
scale/recycle/restart/recovery journeys, or remote-host setup acceptance. A YAML
lint pass is not a real GitHub job result.
