# Server process log viewer

`/logs` is an authenticated administrator-only view of Labby's rolling process logs. It does not display activity or traces as logs.

## Source contract

- `crates/labby/src/dispatch/server_logs/catalog.rs`: service `server_logs`, action `server_logs.query`; read-only, requires administrator authority.
- `dispatch.rs`: `QueryResult` contains `kind: server_logs`, newest-first entries and scan metadata. Entry strings are nullable. The server redacts structured sensitive keys.
- `params.rs`: bounded limit and scan budget, exact level and text query filters.

The console requests at most 200 entries, scans at most 32 KiB, and stops when the limit fills. The existing HTTP transport independently caps returned bodies to 64 KiB; an oversized response fails closed. Only timestamp, level, message, service and target are projected. Unknown fields, raw structured fields, filenames and private log directory paths are omitted. HEEx escapes all text; ANSI CSI styling is stripped rather than interpreted.

Follow performs sequential polling two seconds after a completed request. There is no streaming API claimed here. Each result replaces a bounded window, ordered oldest to newest; repeated queries do not append duplicates. Pause cancels the in-flight request and timer; async cancellation fences late results. Refresh cancels any scheduled timer. Unmount stops the timer and LiveView owns async worker teardown. Errors clear stale content and pause polling. Authentication loss redirects to sign-in.

`LogFollow` scrolls the bounded viewer to the latest line while following; paused viewers keep their position. Shared hook and theme styles are integration-owned.

## Verification

Five focused Elixir regressions passed (seed 908806): query envelope/budget, safe projection, invalid and oversized responses, empty data, caller isolation, revoked access, escaped markup, pause, recovery, deterministic loading, canceled late results, polling disabled while paused, and expiry. ANSI sanitization was added red-first: `/tmp/server-logs-ansi-red.log` showed actual escape sequences failing the expected clean message before implementation; the focused suite then passed.

`e2e/server-logs.spec.mjs` authors a real browser sign-in, long overflowing log window, follow-to-bottom, pause preserving scroll through a polling interval, safe markup rendering, filters, error/empty/recovery, isolated caller, 320px overflow check and polling-driven authentication expiry. The root integrator runs Playwright serially with the other slices; this agent did not run a competing browser suite. No live gateway was queried and no mutations were added.
