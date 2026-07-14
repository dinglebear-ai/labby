# Unraid Plugin

`unraid/` packages labby as a classic Unraid webGUI plugin (`.plg`) — a
native, rc.d-managed process on the Unraid host itself. No Docker, no
systemd (Unraid does not run one). This is a separate deployment target from
[INCUS.md](./INCUS.md) (recommended self-hosted gateway runtime) and the
Docker Compose stack (`docker-compose.prod.yml`); pick whichever fits the
host, they are not mutually exclusive.

## Why native instead of Docker

Unraid has first-class Docker support, and the released image
(`ghcr.io/jmagar/lab`) already runs there. The `.plg` path exists because
"installable as an Unraid plugin" specifically means the classic
`.plg`/Plugins-tab mechanism, and because labby is a single, dynamically
linked (glibc) binary with no bundled shared libraries and its own embedded
admin web UI — it does not need container isolation or a companion frontend
service the way a plugin like `~/workspace/incus-unraid` (packaging Incus,
a system-container manager with real host-library dependencies) does.

Empirically verified on real hardware (Unraid 7.3.1, glibc 2.43): the
`lab-x86_64-unknown-linux-gnu.tar.gz` release binary runs unmodified —
`ldd` resolves cleanly (libc, libgcc_s, libm only) and `labby serve` fully
bootstraps. The binary's max required symbol version is `GLIBC_2.39`, well
under Unraid's `2.43`. No musl/static build is needed for this to work.

## Layout

```
unraid/
  labby.plg                                    plugin manifest (installed via Unraid's Plugins tab)
  source/usr/local/emhttp/plugins/labby/
    labby.cfg                                  default config template (flash-persisted copy is the source of truth once installed)
    Labby.page                                  thin status/control page — links out to labby's own admin UI, does not reimplement one
    scripts/rc.labby                            start/stop/restart/status, mirrors the systemd unit in host_service.rs without depending on systemd
    scripts/labby-preflight.sh                   read-only glibc/binary sanity check; rc.labby refuses to start if this fails
    event/disks_mounted                          array-start hook — labby's state lives on the array, so it can't start before this fires
    event/unmounting_disks                        array-stop hook — stops labby before the array (and LABBY_DIR) unmounts
```

`labby.plg` does not bundle a `.txz`/Slackware package the way
`incus-unraid` does — that exists there because Incus ships multiple
binaries plus host libraries Unraid doesn't provide. labby is one binary
with no extra libs, so the `.plg` downloads the existing GitHub Release
tarball directly (the same asset `scripts/install.sh` consumes) plus each
small companion file under `source/`, each pinned by its own `<MD5>` entity.

## Persistence model (Unraid boots into RAM from a flash drive)

- **Persistent config**: `/boot/config/plugins/labby/labby.cfg` (flash).
  Seeded once at install, never overwritten if already present. Edit
  `SERVICE=enabled` here to autostart on array start.
- **Runtime OS files**: `/usr/local/emhttp/plugins/labby/*` (RAM). Rebuilt
  fresh from the flash-cached tarball + `source/` files on every boot.
- **Gateway state** (`auth.db`, `registry.db`, `config.toml`, the MCP
  bearer token — everything labby normally writes under `$HOME`/XDG dirs):
  `LABBY_DIR` in `labby.cfg`, default `/mnt/user/appdata/labby` (array,
  survives reboots — the same convention every Unraid Docker app's appdata
  mount already uses). `rc.labby` exports `HOME`/`XDG_*` to point there
  instead of root's RAM-only `/root`.

## Two version numbers, on purpose

`labby.plg` tracks two independent versions:

- `version` — the **plugin package's own version**, shown in Unraid's
  Plugins page and used for its install/update comparison. Bumped whenever
  `unraid/` packaging itself changes, even if labby's binary hasn't (e.g.
  the `1.3.0a` bump that shipped the `HTTP_HOST` default fix below).
- `labbyVersion` — the **labby release tag this plugin currently bundles**.
  Only this entity drives `tarballURL`/`tarballMD5`. Keeping it separate
  from `version` means a packaging-only fix never has to point at a labby
  release tag whose binary asset doesn't exist (or force a new labby
  release just to ship a plugin bugfix).

## Keeping the `.plg` in sync with releases

Every `<MD5>` in `labby.plg`, plus `labbyVersion`, must match what's
actually published, or Unraid's install/update either 404s or fails
checksum verification. `scripts/ci/unraid-plugin-checksums.sh` is the single
source of truth for this — it checks (default) or rewrites (`--fix`) every
entity:

```
scripts/ci/unraid-plugin-checksums.sh                                   # check only
scripts/ci/unraid-plugin-checksums.sh --fix                             # repair after editing unraid/source/
scripts/ci/unraid-plugin-checksums.sh --tag vX.Y.Z --tarball PATH       # also check labbyVersion + release tarball MD5
```

- `ci.yml`'s always-on `unraid-plugin-check` job runs the no-args form on
  every push/PR, so editing `unraid/source/` without running `--fix`
  afterward fails CI immediately (this is exactly the failure mode that
  motivated the script — see git history for the `Labby.page` checksum
  drift caught and fixed during initial scaffolding).
- `release.yml`'s `release` job runs the `--tag`/`--tarball` form against
  the tag actually being released, before the GitHub Release is created —
  mirroring the version-matches-tag verify pattern already used for
  `packages/labby-mcp/package.json` and `server.json` in the same workflow.
  It checks `labbyVersion` against the tag, not the plugin's own `version`.
- Neither job auto-commits a fix; a mismatch fails the run and the fix must
  be applied locally (`--fix`) and committed like any other change. This
  was a deliberate choice over having CI push a bot commit back to `main`
  mid-release, to match the rest of this workflow's existing convention and
  avoid a new bot-identity/branch-protection interaction.
- The `version` entity itself has no automated check — it's a plugin-package
  concern bumped by hand, the same way `incus-unraid` hand-bumps its own
  `.plg` version per content change, decoupled from any upstream Incus
  version.

## Known gaps

- No `Icon="labby.png"` asset shipped yet (cosmetic only — Unraid falls
  back to a default icon).
- Not distributed via Community Applications; install via the Plugins tab's
  "Install Plugin" URL field pointed at the raw `labby.plg` URL.
- Not yet validated end-to-end on real hardware through the Unraid Plugins
  page (install/update/uninstall flow, array-start/stop hook behavior across
  a real reboot) — only the underlying binary's runtime compatibility has
  been verified so far.
