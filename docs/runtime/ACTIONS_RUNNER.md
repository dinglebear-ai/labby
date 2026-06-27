# GitHub Actions Self-Hosted Runner Setup

Last updated: 2026-06-27

## Linux self-hosted runner (`linux-lab`) on tootie

CI now runs the full Linux `nextest` lane on a self-hosted runner with labels
`self-hosted` and `linux-lab`.

Fork PRs remain on GitHub-hosted `ubuntu-latest` via `test-fork`.

### Runner target label

- `linux-lab` is recognized in `ci.yml` as the Linux CI label.
- `linux-lab` is listed in `.github/actionlint.yaml` so actionlint accepts the
  custom label.

### Container setup (tootie)

The runner runs as a Docker container on tootie. Keep Compose files on the
cache pool, not under `/opt`, because tootie is Unraid and `/opt` does not
survive reboot.

- Compose: `/mnt/cache/compose/actions-runner/lab/docker-compose.yml`
- Startup script: `/mnt/cache/compose/actions-runner/lab/start.sh`
- Runner state: `/mnt/cache/appdata/actions-runner/lab/`

The container uses GitHub's official runner image and JIT registration. Store a
repo-scoped PAT with runner admin permissions in
`/mnt/cache/compose/actions-runner/lab/.env`:

```bash
GITHUB_PAT=github_pat_or_gho_token_here
```

Current Compose shape:

```yaml
services:
  lab-linux-runner:
    image: ghcr.io/actions/actions-runner:2.335.1
    container_name: lab-linux-runner
    restart: unless-stopped
    working_dir: /home/runner
    environment:
      - RUNNER_REPO=jmagar/lab
      - RUNNER_NAME=tootie-lab-linux
      - RUNNER_LABELS=linux-lab,self-hosted,linux,x64
      - RUNNER_WORKDIR=/home/runner/_work
      - RUNNER_URL=https://github.com/jmagar/lab
      - RUNNER_USE_JIT=1
      - TMPDIR=/tmp
      - TMP=/tmp
      - TEMP=/tmp
      - RUNNER_TEMP=/home/runner/_work/_temp
    env_file:
      - .env
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - /mnt/cache/appdata/actions-runner/lab/home:/home/runner
      - /mnt/cache/appdata/actions-runner/lab/work:/home/runner/_work
      - /mnt/cache/appdata/actions-runner/lab/tmp:/tmp
      - /mnt/cache/compose/actions-runner/lab/start.sh:/start.sh:ro
    command: ["/start.sh"]
```

Start or restart from the persistent Compose directory:

```bash
cd /mnt/cache/compose/actions-runner/lab
docker compose up -d
```

The startup script removes stale same-name remote runners, generates a JIT
config through GitHub's API, and runs `./run.sh --jitconfig ...`. It also keeps
container temp usage cache-backed by bind-mounting container `/tmp` to
`/mnt/cache/appdata/actions-runner/lab/tmp`, avoiding Unraid's RAM-backed host
`/tmp`.

### Validation

```bash
cd /mnt/cache/compose/actions-runner/lab
docker compose logs -f
docker exec lab-linux-runner df -h /tmp /home/runner /home/runner/_work
```

From GitHub, confirm the runner is online with labels:

- `self-hosted`
- `linux-lab`

### Notes

- The runner uses JIT registration. If it goes offline, restart the Compose
  service; `start.sh` removes the stale remote runner and registers a fresh one.
- Do not bind-mount an empty directory over `/home/runner` without seeding the
  runner image contents first; that hides `run.sh` and `config.sh`.
- Keep this runner label in `.github/actionlint.yaml` and in `ci.yml` whenever
  labels change.
- If you want strict hardening, add container resource limits and restart policy
  controls in a systemd unit wrapping Compose.
