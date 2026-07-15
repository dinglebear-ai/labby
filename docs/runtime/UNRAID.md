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
(This was a one-off binary-compatibility check on a production NAS, done
before plugin development started. All later end-to-end plugin testing —
install/start/stop/settings-form/OAuth-login flows — moved to a dedicated
test box, `tower` (Unraid 7.3.2); see "Validated end-to-end" below.)

## Layout

```
unraid/
  labby.plg                                    plugin manifest (installed via Unraid's Plugins tab)
  source/usr/local/emhttp/plugins/labby/
    labby.cfg                                  default config template (flash-persisted copy is the source of truth once installed)
    Labby.page                                  status + settings form (SERVICE/LABBY_DIR/HTTP_HOST/HTTP_PORT) — links out to labby's own admin UI rather than reimplementing one
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

## Settings page conventions

`Labby.page` is a real settings form (SERVICE, LABBY_DIR, HTTP_HOST,
HTTP_PORT — everything in `labby.cfg`), built to look and behave like a
first-party classic Unraid settings page rather than a custom-styled form.
The markup conventions were reverse-engineered from a live Unraid 7.3.x
install's own pages (`/usr/local/emhttp/webGui/DateTime.page`,
`dynamix.my.servers/Connect.page`) and cross-checked against
`~/workspace/upstream/unraid-api` (which shares the same theme-token
contract with the classic webGUI via `Theme--white/black/gray/azure`, even
though its Tailwind/Vue tokens aren't directly usable from a classic
`.page`):

- **Fields**: the `_(Label)_:` / `: <input>` markdown definition-list idiom
  on a `<form markdown="1">` — Unraid's own page renderer
  (`PageBuilder.php`/`MainContent.php`) runs the whole page body through
  `Markdown()`, which turns this into a real `<dl>`. No custom wrapper divs.
- **Selects**: `mk_option($current, $value, $label)` (a core webGUI global,
  loaded unconditionally via `webGui/template.php`) — used for every
  enum-valued field (SERVICE, HTTP_HOST), matching how every real
  first-party page handles booleans/enums (no switchbutton widget; that's
  a real but unconfirmed-markup asset pulled from a separate `unraid/webgui`
  repo at dev time, not worth the risk of hand-rolling incorrectly).
- **Help text**: `<blockquote class="inline_help">` immediately after a
  field's `dt`/`dd` — wired automatically to the toolbar's "?" help toggle
  by core JS (`DefaultPageLayout.php`) that scans for this class, no extra
  markup needed on the label side.
- **Status color**: `.green-text`/`.red-text` (core classes backed by
  `--green-800`/`--red-600`), not hand-rolled hex values.
- **Buttons**: bare `<input type="submit">` — no `.btn`-style class exists
  in any genuine first-party `.page`; the core stylesheet styles submit/
  button inputs automatically.
- **Icon**: `Icon="server"` — a bare, unprefixed FontAwesome name. Unraid's
  icon resolver (`DefaultPageLayout.php`) auto-prepends `fa-` for any
  `Icon=` value that isn't `icon-*` (Unraid's own reserved webfont family)
  or a `.png` filename — this is the correct zero-asset choice until a real
  `labby.png` exists under `source/.../icons/`.

**Persistence stays custom, deliberately**: settings do *not* go through
Unraid's generic `Dispatcher.php` (`POST /update.htm` with a `#cfg` field),
even though that's the more "native" save mechanism for classic pages.
`Dispatcher.php` requires an INI `[section]`-headed config file
(`parse_ini_file($file, true)`), but `labby.cfg` is a flat, `[section]`-free
`KEY="value"` file so `rc.labby`/the event hooks can keep bash-`source`-ing
it unmodified. `Labby.page` POSTs back to itself instead and regenerates
the whole file from a template, preserving the explanatory comments.

Because `rc.labby` bash-sources `labby.cfg` verbatim, **every field is
validated server-side against a strict allowlist before being written** —
an unvalidated value containing a `"`, `$`, backtick, or newline would be
interpreted as shell syntax the next time the file is sourced, not just a
bad config value. SERVICE/HTTP_HOST are checked against an exact-match
enum (not just constrained by the `<select>`, which a crafted raw POST can
bypass), HTTP_PORT against a numeric 1–65535 range, and LABBY_DIR against
a path-character allowlist (`^/[A-Za-z0-9_./-]+$`) with no shell
metacharacters permitted at all.

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
- The `--tag`/`--tarball` form is a **manual tool, not wired into any CI
  job**. An earlier version of this PR ran it automatically from
  `release.yml` against `${{ github.ref_name }}` (the tag currently being
  released) — that was wrong on two counts, caught in review: `labbyVersion`
  intentionally pins to a specific, already-published, manually-vetted
  labby release (see "Two version numbers" above), not whatever tag is
  currently being built, so comparing it to `github.ref_name` would fail on
  every release where they legitimately differ (which is the normal case);
  and even if they matched, a freshly-built release tarball's MD5 is not
  reproducible build-to-build — GNU tar embeds each packaged file's mtime,
  so byte-identical binary content still produces a different archive hash
  on every CI run, making an automatic same-run comparison impossible to
  ever pass. Run this form by hand instead, against a tarball downloaded
  from the already-published release you're pointing `labbyVersion` at,
  whenever you deliberately bump it:
  ```
  gh release download vX.Y.Z --repo jmagar/labby -p "lab-x86_64-unknown-linux-gnu.tar.gz"
  scripts/ci/unraid-plugin-checksums.sh --tag vX.Y.Z --tarball lab-x86_64-unknown-linux-gnu.tar.gz --fix
  ```
- The `unraid-plugin-check` CI job does not auto-commit a fix; a mismatch
  fails the run and the fix must be applied locally (`--fix`) and committed
  like any other change.
- The `version` entity itself has no automated check — it's a plugin-package
  concern bumped by hand, the same way `incus-unraid` hand-bumps its own
  `.plg` version per content change, decoupled from any upstream Incus
  version.

### Required step: tag every commit that touches `labby.plg` or `unraid/source/`

`srcURL` (companion-file downloads: `labby.cfg`, `Labby.page`, `rc.labby`,
`labby-preflight.sh`, both event hooks) is pinned to an immutable tag —
`unraid-v&version;` — not to `main`. This is deliberate: every file under
`srcURL` is MD5-verified against a value baked into whatever `version` is
cached on flash, and Unraid's classic `.plg` model re-downloads and
re-verifies every `<FILE>` on **every boot**, since `/usr/local/emhttp` is
tmpfs and gets wiped on reboot. If `srcURL` pointed at `main` (as it did
before `1.3.0e`), any later commit touching these files — even for a
totally unrelated packaging round — would break every already-installed
copy's next boot with an MD5 mismatch, without `version` ever having
changed for that install. `pluginURL` (the manifest URL Unraid's plugin
manager polls to detect updates) deliberately stays on `main` — that one
must always resolve to the latest content, or update detection would freeze.

After committing any change to `labby.plg` or `unraid/source/`:

```
git tag unraid-v<version>   # e.g. unraid-v1.3.0e, matching the version entity
git push origin unraid-v<version>
```

This has no automated check — the tag legitimately can't exist until after
the commit it points at is pushed, so `scripts/ci/unraid-plugin-checksums.sh`
cannot verify it in the same CI run. Forgetting this step doesn't break the
commit you just made (fresh installs and installs already on `main`'s
current state at push time still resolve `srcURL` correctly), but it does
mean the NEXT commit that touches these files will retroactively break any
install that adopted the untagged version — tag every round, not just when
something feels risky.

## Known gaps

- No `labby.png` icon asset yet — `Icon="server"` (a bare FontAwesome name)
  is the interim zero-asset choice, not a broken/missing reference.
- Not distributed via Community Applications; install via the Plugins tab's
  "Install Plugin" URL field pointed at the raw `labby.plg` URL.
- Validated end-to-end on real hardware (tower, Unraid 7.3.2) via Unraid's
  actual `plugin` command: fresh install (checksum-verified download of
  every file), `rc.labby` start/status/ready/stop, `plugin remove
  labby.plg` uninstall (state correctly preserved), a real version-bump
  update cycle (`plugin check`/`plugin update`), the settings form's
  save-and-regenerate path (including injection-resistance testing), and a
  real Google OAuth login through the web UI (via a Tailscale Serve HTTPS
  front). **Not yet tested**: the `event/disks_mounted` and
  `event/unmounting_disks` hooks via a real array stop/start (only
  `rc.labby start`/`stop` have been invoked directly), and reboot
  persistence (the RAM-boot rebuild-from-flash-cache behavior has never
  actually been observed across a real reboot).
