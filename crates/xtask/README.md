# xtask

Repository automation binary for developer measurements and proxy verification.
It is internal (`publish = false`), has no optional features, and does not own
product runtime behavior. [main.rs](src/main.rs) dispatches exactly two CLI
subcommands: `codemode-bench` and `proxy-verify`.

Run from the Labby repository root with its pinned
[Rust toolchain](../../rust-toolchain.toml). Relative binary/output paths resolve
from the invoking working directory. Proxy verification also calls Git to find
the checkout root and runs its nested Cargo commands there. There is no public
library API or additional `cargo xtask` alias in the repository configuration.

## Code Mode benchmark

`codemode-bench` drives a supplied executable's `internal code-mode-runner`
subcommand over newline-delimited JSON. Build or select that executable before
running the benchmark; this command does not build it. The executable must
implement the runner protocol described by [labby-codemode](../labby-codemode/README.md).

```bash
cargo run -p xtask -- codemode-bench \
  --binary target/debug/labby --label local --case warm_return \
  --iterations 30 --warmup 5 --json
```

| Option | Behavior |
| --- | --- |
| `--binary PATH` | Required path to the runner-capable executable. |
| `--label NAME` | Result label; defaults to the executable's filename. |
| `--iterations N` | Measured starts per case; default 30. Use a nonzero count for meaningful statistics. |
| `--warmup N` | Additional unreported starts per case; default 5, with zero disabling warmup. |
| `--case NAME` | Repeat to select cases; omission runs all four in the order below. |
| `--timeout-ms N` | Timeout value passed into the harness/frame; default 10,000 ms. It does not enforce a read deadline. |
| `--json` | Emits a pretty JSON object with a `results` array instead of text rows. |
| `-h`, `--help` | Prints benchmark usage. |

| Case | Work | Starts per runner process |
| --- | --- | --- |
| `cold_return` | Returns 42. | 1 |
| `warm_return` | Returns 42 repeatedly. | Up to 10 |
| `tool_fanout` | Fans out 20 synthetic `bench::echo` calls and returns their count. | Up to 5 |
| `large_result` | Returns 1,000 objects with short text fields. | Up to 5 |

Warmup and measurement use separate invocations of the case runner, so warmup
does not leave a process alive for measured samples. Each sample times writing
`Start` through receiving `Done`; the timer begins after `Command::spawn`.
Process launch itself is not a separately measured metric. Process reuse still
uses a fresh JS runtime per start in the production runner.

The harness supplies synthetic `ToolResult` values, artifact-write replies, and
snippet resolutions. It sends an empty proxy catalog and does not exercise
gateway discovery, upstream latency, authorization, production pool admission,
or real artifact storage. A successful run closes stdin, waits up to five
seconds for exit, then kills/waits if necessary.

`read_frame` uses blocking `read_line` and ignores its timeout argument. The
serialized `timeout_ms` is not a substitute for the host driver's deadline;
the command has no enforced overall deadline. Use separately bounded execution
when collecting measurements. Text rows report median, p95, mean, and count;
JSON also includes minimum, maximum, label, and the supplied binary path. That
benchmark JSON does not go through the proxy proof redaction scanner.

## Proxy verifier

[proxy_verify.rs](src/proxy_verify.rs) exercises a real Labby proxy against the
`stdio-mcp-fixture` binary and runs focused product test commands. It needs
Cargo, Git, curl, a proxy-capable Labby binary, and the relevant product-test
prerequisites. Unix cleanup probes use `kill`; non-Unix PID residue detection
does not perform that Unix liveness check.

```bash
TMPDIR=/absolute/owned/private/tmp cargo run -p xtask -- proxy-verify \
  --binary target/debug/labby --output /absolute/owned/proxy-proof --json
```

Create the private temporary parent first. Proxy verification creates scratch,
can build its fixture, runs test suites, opens local listeners, and replaces
the four known proof files in the selected output directory.

| Option | Behavior |
| --- | --- |
| `--binary PATH` | Defaults to `CARGO_TARGET_DIR/debug/labby`, or `target/debug/labby` when unset. Supply an explicit platform executable path when needed. |
| `--output DIR` | Proof directory; default `target/proxy-proof`, independent of `CARGO_TARGET_DIR`. |
| `--keep-temp` | Retains verifier scratch for diagnosis. |
| `--json` | Prints a final compact result/manifest summary. Nested Cargo commands can still write output before it. |
| `-h`, `--help` | Prints options and the live opt-in mechanism. |

The fixture must be a sibling of the selected binary, named
`stdio-mcp-fixture` (`.exe` on Windows). If missing, the verifier attempts
`cargo build -p labby --all-features --features proxy-testkit --bins --locked`
in the checkout. Its resulting fixture must still exist beside the selected
binary; a custom binary directory does not change Cargo's output directory.

The fixture suite checks local random-port/no-auth and fixed-port/bearer
proxies, tools/resources/prompts passthrough, hostile Host/Origin rejection,
diagnostic canaries, and fixture-child cleanup. It then runs product command
resolution tests, `stdio_proxy_runtime`, fake `tailscale_serve`, and runtime
unit fault tests with `proxy-testkit`. The integration targets run serially.
These selected Cargo tests are part of this command, not an xtask-only unit run.

Direct probes allow 15 seconds for readiness. Shutdown sends SIGINT on Unix
(kills on other platforms), waits up to eight seconds, and then kills/waits if
needed. Scratch lives under `std::env::temp_dir()` in a PID-named directory;
creation replaces an existing same-name directory. Drop removes scratch on a
best-effort basis unless `--keep-temp` is set. Use an owned private `TMPDIR` and
retain responsibility for kept files; the code does not set directory mode
`0700` itself. The verifier does not impose a hard deadline on every subprocess
or HTTP operation.

## Proof files and live boundary

| Output | Contents |
| --- | --- |
| `manifest.json` | `labby.proxy-proof` version 1, checkout commit/tree state, scenario outcomes, live mode, and artifact SHA-256 values. |
| `commands.jsonl` | Stable scenario command labels, exit status, and bounded evidence descriptions. |
| `summary.md` | Overall result and passed/skipped/failed counts. |
| `redaction-scan.json` | Scanner result and the classes it checked. |

Proof scanning rejects known canaries, user/home/temp values, selected dynamic
structured fields, process identifiers, and opaque lease-ID shapes. It checks
all files already present in the proof directory too. These normalized records
do not retain raw subprocess logs. Exit status is 0 when no scenario failed,
1 on verification/runtime failure, and 2 on option/usage errors. Some early
setup failures can occur before a complete manifest is written.

Only `LABBY_PROXY_LIVE=1` enables the real Tailscale gate. It requires the
Tailscale CLI, a usable signed-in node, HTTPS Serve configuration, and
authority to create/remove the temporary Serve mapping. It starts a tailnet
proxy to the fixture, requests HTTPS `tools/list`, stops the proxy, and compares
optional parsed Serve status before/after. Unavailable status reads can both
be absent, so equality alone is limited cleanup evidence. With live mode off,
the manifest marks that scenario `skipped`; fixture success does not qualify a
live tailnet, deployment, or production authentication configuration.

## Development and module reachability

`module_reachability` is compiled only for tests. The supported gate is
`just module-reachability`, which runs `cargo test -p xtask module_reachability`.
It checks module declarations/path attachments for files under `crates/*/src`,
including directories lacking an owning module. It does not inspect separate
Rust trees such as the Tauri workspace and is not a CLI subcommand.

Focused crate checks are `cargo nextest run -p xtask` and
`cargo clippy -p xtask --all-targets -- -D warnings`. CLI tests cover proxy help,
sanitized early failure, and normalized proof output; they do not execute the
full proxy/live workflow. Follow [crate instructions](AGENTS.md),
[testing](../../docs/dev/TESTING.md), the
[proxy guide](../../docs/guides/STDIO_MCP_PROXY.md), and the
[proxy contract](../../docs/contracts/stdio-mcp-proxy.md) for owning behavior.
