# labby-web

Host-neutral static asset lookup for Labby's exported Gateway Admin UI. This
crate returns bytes and header metadata; the product host owns routing,
authentication, node-role policy, HTTP methods, and response construction.
It has no Axum dependency and does not start an HTTP or Node server.

The public API lives in [lib.rs](src/lib.rs). The implementation separates
[embedded assets](src/assets.rs), [filesystem lookup](src/fs_assets.rs), and
[response metadata](src/response.rs). The
[product adapter](../labby/src/api/web.rs) applies the UI route allowlist,
selects a source, maps failures to 404, and removes bodies for `HEAD`.

## API and integration

| API | Contract |
| --- | --- |
| `AssetSource::Embedded` | Uses the bytes captured during this crate's build. |
| `AssetSource::Directory(PathBuf)` | Resolves and reads files from the selected directory for each request. |
| `AssetSource::from_configured_dir_or_embedded(dir)` | Selects `Directory` for `Some`; selects `Embedded` for `None`. |
| `serve_asset(path, source).await` | Returns `AssetResponse` or `AssetError::NotFound`. |
| `AssetResponse` | Owns `bytes: Vec<u8>`, plus static `content_type` and `cache_control` strings. |
| `resolve_fs_asset` / `resolve_embedded_asset` | Expose lower-level path/byte resolution without transport responses. |
| `embedded_asset`, `embedded_asset_paths` | Look up an exact relative bundle key or enumerate embedded paths. |
| `embedded_assets_available` | Checks whether the embedded `index.html` key exists. |

An async host can select its source and obtain the root page as follows:

```rust
use std::path::PathBuf;
use labby_web::{AssetError, AssetResponse, AssetSource, serve_asset};

async fn root_page(dir: Option<PathBuf>) -> Result<AssetResponse, AssetError> {
    let source = AssetSource::from_configured_dir_or_embedded(dir);
    serve_asset("/", &source).await
}
```

The caller passes a URI path without its query string, authenticates and routes
the request before lookup, and maps the returned metadata into HTTP headers.
`AssetResponse` always includes the complete body, even for a host handling
`HEAD`; the host must omit that body. Directory reads use Tokio filesystem I/O.
A missing export or missing root page can make the example return `NotFound`.

## Source selection and path resolution

A configured directory is the sole selected source. A missing directory/file
does not fall through to embedded bytes. Source selection is explicit product
policy; the helper constructor only encodes that choice.

`sanitize_relative_path` removes leading `/`, ignores `.` components, and
collapses traversal, root, or platform-prefix components to an empty path.
Empty paths resolve to `index.html`, so a sanitized-away path can resolve the
root page rather than producing a distinct invalid-path error. The function
does not perform URL decoding or make routing/authorization decisions.

| Source | Resolution order |
| --- | --- |
| Embedded | Exact sanitized key, then `<path>/index.html`, then root `index.html` only for an extensionless miss. |
| Directory | An existing file serves itself; an existing directory selects its own `index.html`; a missing extensionless candidate selects root `index.html`. |

The directory-backed resolver does not retry the root page when an existing
directory lacks its own index. Missing file-like requests such as `/missing.js` and `/install.sh`
return `NotFound` instead of the root SPA page. A directory index remains a
valid match when present, including for a directory whose name contains a dot.

Filesystem resolution canonicalizes both the configured root and the chosen
candidate and rejects a candidate outside the canonical root. Symlinks that
remain inside the root can resolve; symlink escapes, canonicalization failures,
and read failures map to `NotFound`. Header metadata uses the resolved path,
including the canonical target filename for directory-backed assets.

## Content types and cache policy

`guess_content_type` maps lowercase extensions to HTML, CSS, shell script,
JavaScript, JSON, SVG, common image formats, text, and WOFF/WOFF2 font types.
Source maps use `application/json`; unknown extensions use
`application/octet-stream`. Textual HTML, CSS, shell, JavaScript, and text
responses include UTF-8 charset metadata. The helper does not inspect contents.

`cache_control_for` uses the resolved filename/extension:

| Resolved asset | `Cache-Control` |
| --- | --- |
| Any basename `index.html` or `install.sh` | `no-store` |
| Any `.txt`, including Next.js route payloads | `no-store` |
| Every other path | `public, max-age=31536000, immutable` |

The immutable rule does not verify content hashes. It also applies to an HTML
file whose basename is not `index.html`; do not infer cache policy from the
HTML content type. Keep mutable bootstrap artifacts aligned with this explicit
policy when adding exports. The product adapter preserves these headers.

## Embedded bundle lifecycle

[build.rs](build.rs) reads the Next.js static export in `apps/web/out`, collects
files recursively, normalizes relative keys to forward slashes, sorts them,
and emits `embedded_web_assets.rs` in Cargo's `OUT_DIR`. The table uses
`include_bytes!` for each file. It watches the asset tree and files for changes,
or the nearest existing parent when the export directory is absent.

The build script runs before Rust compilation, keeping directory traversal out
of remote macro expansion. An absent export emits a warning and an empty table;
this is a supported backend-only build. A root that is not a directory, root
access failure, or directory-walk error fails the build. An empty table serves
no UI. The presence check only establishes an embedded `index.html`, not a
fresh export or successful browser rendering.

This crate has only an empty default feature set; there are no optional asset
backends to enable. The product's `web-ui` feature controls its dependency and
adapter. Rust-only builds need the pinned [toolchain](../../rust-toolchain.toml)
and do not need Node when an export is absent. Frontend source, Node/pnpm pins,
and export setup belong to [Gateway Admin](../../apps/web/README.md).

## Focused verification

From the repository root:

```bash
cargo nextest run -p labby-web
cargo clippy -p labby-web --all-targets -- -D warnings
```

Embedded-asset verification requires a fresh frontend export before the Rust
build/test invocation. The root `just web-build` recipe installs the locked
frontend dependencies and builds that export; raw Cargo does not refresh it.
Tests cover filesystem serving, SPA/file-like fallbacks, symlink escape
rejection, content types, and cache metadata. Embedded tests return early when
their required bundle assets are absent; a passing backend-only suite does not
prove that the UI was embedded. Browser behavior and authenticated host routes
need their own owning-surface checks.

See [crate instructions](AGENTS.md), [testing](../../docs/dev/TESTING.md), and
[frontend CI](../../docs/runtime/CICD.md#frontend-tests) for verification scope.
Follow [documentation maintenance](../../docs/dev/DOCUMENTATION.md) when changing
the owning source or generated documentation.
