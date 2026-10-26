# Farm runner acceptance review

The activation candidate is `.github/workflows/farm-runner-smoke.yml` in the
owned local task branch. Nothing is pushed, installed remotely, or dispatched.
The explicit user exception and push approval are still pending at preparation. Labby's `.github/AGENTS.md` and CI contract require GitHub-hosted
product CI. Activating this manual acceptance workflow needs a reviewed explicit
exception, authorized push and human merge; it must not reroute existing jobs or modify
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

GitHub only exposes `workflow_dispatch` after the workflow exists on the default
branch. Pushing this task branch or opening a draft PR does not make it runnable.
Keep human merge and the later bounded live runner/dispatch authorization separate.
