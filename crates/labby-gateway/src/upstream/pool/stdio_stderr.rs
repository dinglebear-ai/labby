//! Stdio upstream stderr capture, logging, and startup diagnostics.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Mutex;

const STDERR_DIAGNOSTIC_MAX_LINES: usize = 80;

#[derive(Clone, Default)]
pub(super) struct StdioDiagnostics {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl StdioDiagnostics {
    async fn push(&self, line: String) {
        let mut lines = self.lines.lock().await;
        if lines.len() >= STDERR_DIAGNOSTIC_MAX_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub(super) async fn snapshot(&self) -> String {
        // Give the stderr drain task a short chance to flush lines emitted right
        // before the child exited. This is intentionally tiny; startup failure
        // should still return promptly.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.lines
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(super) struct StdioConnectError {
    error: anyhow::Error,
    diagnostics: String,
    /// The child closed its stdout (transport EOF) before the connection was
    /// established. Such a failure says nothing about lifecycle compatibility.
    child_exited: bool,
}

impl StdioConnectError {
    pub(super) fn without_diagnostics<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            error: anyhow::Error::new(error),
            diagnostics: String::new(),
            child_exited: false,
        }
    }

    pub(super) async fn with_diagnostics<E>(
        error: E,
        diagnostics: &StdioDiagnostics,
        child_exited: bool,
    ) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        let diagnostics = diagnostics.snapshot().await;
        Self {
            error: anyhow::Error::new(error),
            diagnostics,
            child_exited,
        }
    }

    #[must_use]
    pub(super) const fn child_exited(&self) -> bool {
        self.child_exited
    }

    /// The typed MCP-level failure on its own, with the child's stderr excluded.
    ///
    /// Lifecycle-compatibility classification must read this and never
    /// [`Self::diagnostics_with_error`]: the stderr tail is the child's own log
    /// output, and an ordinary server log line ("Error: Method not found") is
    /// not the peer rejecting `server/discover`. Matching protocol vocabulary
    /// against log noise downgrades healthy upstreams and respawns them. Keeping
    /// the original error also preserves fail-closed protocol error codes for
    /// compatibility logic. Operator-facing text, cache-poison repair, and logs
    /// still use the full diagnostics.
    #[must_use]
    pub(super) const fn protocol_error(&self) -> &anyhow::Error {
        &self.error
    }

    pub(super) fn diagnostics_with_error(&self) -> String {
        let message = format!("{:#}", self.error);
        if self.diagnostics.trim().is_empty() {
            message
        } else {
            format!("{message}\n{}", self.diagnostics)
        }
    }

    pub(super) fn into_anyhow(self) -> anyhow::Error {
        if self.diagnostics.trim().is_empty() {
            self.error
        } else {
            self.error
                .context(format!("upstream stderr:\n{}", self.diagnostics))
        }
    }
}

/// `[gateway].upstream_stderr_level` from `config.toml`, seeded once by
/// `install_upstream_stderr_level_default` at config load time (see
/// `GatewayManager::reload_with_origin_unlocked`). Consulted as a fallback
/// below the env var.
static UPSTREAM_STDERR_LEVEL_CONFIG_DEFAULT: std::sync::OnceLock<Option<String>> =
    std::sync::OnceLock::new();

/// Seed the config.toml fallback for `upstream_stderr_log_level()`. Safe to
/// call more than once (e.g. on every `gateway.reload`) — later calls are a
/// no-op once the first value is set, since `LABBY_GW_UPSTREAM_STDERR` is
/// itself resolved fresh on every call and takes precedence regardless.
pub(crate) fn install_upstream_stderr_level_default(value: Option<String>) {
    drop(UPSTREAM_STDERR_LEVEL_CONFIG_DEFAULT.set(value));
}

/// Resolve the log level for forwarded upstream stderr.
///
/// Priority: `LABBY_GW_UPSTREAM_STDERR` env var > `config.toml`
/// `[gateway].upstream_stderr_level` > default (`debug`).
pub(super) fn upstream_stderr_log_level() -> Option<tracing::Level> {
    let raw = std::env::var("LABBY_GW_UPSTREAM_STDERR").ok().or_else(|| {
        UPSTREAM_STDERR_LEVEL_CONFIG_DEFAULT
            .get()
            .cloned()
            .flatten()
    });
    parse_stderr_level(raw.as_deref())
}

fn parse_stderr_level(raw: Option<&str>) -> Option<tracing::Level> {
    let Some(raw) = raw else {
        return Some(tracing::Level::DEBUG);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "null" | "off" | "0" | "none" | "discard" | "false" => None,
        "trace" => Some(tracing::Level::TRACE),
        "info" => Some(tracing::Level::INFO),
        "warn" | "warning" => Some(tracing::Level::WARN),
        // "debug", enable-flavored values, and anything unrecognized fall back
        // to the default level.
        _ => Some(tracing::Level::DEBUG),
    }
}

/// Truncate `line` to at most `max` bytes without splitting a UTF-8 codepoint.
fn cap_line_bytes(line: &str, max: usize) -> &str {
    if line.len() <= max {
        return line;
    }
    let mut cut = max;
    while cut > 0 && !line.is_char_boundary(cut) {
        cut -= 1;
    }
    &line[..cut]
}

/// Maximum number of bytes forwarded per line from a child's stderr.
const STDERR_LINE_MAX_BYTES: usize = 1024;

/// Maximum number of lines forwarded per second from a single upstream's stderr.
const STDERR_RATE_CAP_PER_SEC: u32 = 50;

/// Retain only a diagnostic prefix while draining the rest of an oversized
/// line. Emit the capped prefix immediately so a child cannot hold an
/// unbounded line in memory (or withhold startup diagnostics) without a newline.
struct BoundedStderrLines<R> {
    reader: tokio::io::BufReader<R>,
    line: Vec<u8>,
    discarding: bool,
}

impl<R: tokio::io::AsyncRead + Unpin> BoundedStderrLines<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: tokio::io::BufReader::new(reader),
            line: Vec::with_capacity(STDERR_LINE_MAX_BYTES),
            discarding: false,
        }
    }

    async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        use tokio::io::AsyncBufReadExt;

        loop {
            let buffer = self.reader.fill_buf().await?;
            if buffer.is_empty() {
                return Ok((!self.line.is_empty()).then(|| self.take_line(false)));
            }
            let mut consumed = 0;
            let mut complete = None;
            for &byte in buffer {
                consumed += 1;
                if self.discarding {
                    if byte == b'\n' {
                        self.discarding = false;
                    }
                    continue;
                }
                if byte == b'\n' {
                    complete = Some(false);
                    break;
                }
                if self.line.len() == STDERR_LINE_MAX_BYTES {
                    self.discarding = true;
                    complete = Some(true);
                    break;
                }
                self.line.push(byte);
            }
            self.reader.consume(consumed);
            if let Some(truncated) = complete {
                return Ok(Some(self.take_line(truncated)));
            }
        }
    }

    fn take_line(&mut self, truncated: bool) -> String {
        if !truncated && self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        let decoded = String::from_utf8_lossy(&self.line);
        let capped = cap_line_bytes(&decoded, STDERR_LINE_MAX_BYTES);
        let line = if truncated {
            format!("{capped}…[truncated]")
        } else {
            capped.to_string()
        };
        self.line.clear();
        line
    }
}

/// Drain a piped child stderr to EOF, forwarding non-empty lines into tracing
/// and retaining a bounded redacted tail for startup-failure diagnosis.
pub(super) fn forward_upstream_stderr(
    stderr: Option<tokio::process::ChildStderr>,
    upstream: String,
    level: Option<tracing::Level>,
    diagnostics: StdioDiagnostics,
) {
    let Some(stderr) = stderr else {
        tracing::warn!(
            target: "labby::upstream_stderr",
            upstream = %upstream,
            "stderr capture enabled but child returned no stderr handle; upstream diagnostics will be lost"
        );
        return;
    };
    tokio::spawn(async move {
        use labby_runtime::redact::redact_stdio_value;
        let mut lines = BoundedStderrLines::new(stderr);
        let mut window_start = std::time::Instant::now();
        let mut lines_this_window: u32 = 0;
        let mut dropped_this_window: u32 = 0;

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }

                    if window_start.elapsed().as_secs() >= 1 {
                        if dropped_this_window > 0 {
                            tracing::warn!(
                                target: "labby::upstream_stderr",
                                upstream = %upstream,
                                dropped = dropped_this_window,
                                "upstream stderr rate cap exceeded; lines dropped"
                            );
                        }
                        window_start = std::time::Instant::now();
                        lines_this_window = 0;
                        dropped_this_window = 0;
                    }

                    if lines_this_window >= STDERR_RATE_CAP_PER_SEC {
                        dropped_this_window += 1;
                        continue;
                    }
                    lines_this_window += 1;

                    let redacted = redact_stdio_value(&line);
                    diagnostics.push(redacted.clone()).await;

                    let Some(level) = level else {
                        continue;
                    };

                    macro_rules! emit {
                        ($macro:ident) => {
                            tracing::$macro!(
                                target: "labby::upstream_stderr",
                                surface = "dispatch",
                                service = "upstream.pool",
                                upstream = %upstream,
                                stream = "stderr",
                                "{redacted}",
                            )
                        };
                    }
                    match level {
                        tracing::Level::TRACE => emit!(trace),
                        tracing::Level::INFO => emit!(info),
                        tracing::Level::WARN | tracing::Level::ERROR => emit!(warn),
                        _ => emit!(debug),
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::debug!(
                        target: "labby::upstream_stderr",
                        upstream = %upstream,
                        error = %error,
                        "upstream stderr drain ended on read error",
                    );
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{cap_line_bytes, parse_stderr_level};

    #[cfg(unix)]
    #[tokio::test]
    async fn oversized_open_stderr_line_is_bounded_before_newline() {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        // The child confirms it wrote the oversized line, then waits on stdin.
        // Waiting for a newline here would retain arbitrary child output.
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "i=0; while [ $i -lt 2048 ]; do printf x >&2; i=$((i+1)); done; printf 'ready\\n'; read release; printf '\\nnext-line\\n' >&2"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("stderr fixture");
        let diagnostics = super::StdioDiagnostics::default();
        super::forward_upstream_stderr(
            child.stderr.take(),
            "bounded-stderr-fixture".into(),
            None,
            diagnostics.clone(),
        );
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout")).lines();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("ready"));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !diagnostics.lines.lock().await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("oversized stderr must be capped while the line remains open");
        {
            let lines = diagnostics.lines.lock().await;
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0], format!("{}…[truncated]", "x".repeat(1024)));
        }
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        assert!(child.wait().await.unwrap().success());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if diagnostics
                    .lines
                    .lock()
                    .await
                    .back()
                    .is_some_and(|line| line == "next-line")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drain must recover at the next line");
        assert_eq!(diagnostics.lines.lock().await.len(), 2);
    }

    #[test]
    fn stderr_level_unset_defaults_to_debug() {
        assert_eq!(parse_stderr_level(None), Some(tracing::Level::DEBUG));
    }

    #[test]
    fn stderr_level_named_levels_parse() {
        assert_eq!(
            parse_stderr_level(Some("trace")),
            Some(tracing::Level::TRACE)
        );
        assert_eq!(
            parse_stderr_level(Some("debug")),
            Some(tracing::Level::DEBUG)
        );
        assert_eq!(parse_stderr_level(Some("INFO")), Some(tracing::Level::INFO));
        assert_eq!(
            parse_stderr_level(Some(" warn ")),
            Some(tracing::Level::WARN)
        );
        assert_eq!(
            parse_stderr_level(Some("warning")),
            Some(tracing::Level::WARN)
        );
    }

    #[test]
    fn stderr_level_disable_values_discard() {
        for raw in ["null", "off", "0", "none", "discard", "FALSE"] {
            assert_eq!(parse_stderr_level(Some(raw)), None, "{raw}");
        }
    }

    #[test]
    fn stderr_level_enable_flavored_and_unknown_fall_back_to_debug() {
        for raw in ["", "on", "1", "true", "verbose", "garbage"] {
            assert_eq!(
                parse_stderr_level(Some(raw)),
                Some(tracing::Level::DEBUG),
                "{raw}"
            );
        }
    }

    #[test]
    fn cap_line_bytes_is_utf8_boundary_safe() {
        let line = "aéé";
        assert_eq!(cap_line_bytes(line, 3), "aé");
        assert_eq!(cap_line_bytes("abc", 8), "abc");
        assert_eq!(cap_line_bytes("éé", 4), "éé");
        assert_eq!(cap_line_bytes("ééé", 1), "");
    }

    #[test]
    fn cap_line_bytes_never_panics_on_any_boundary() {
        let line = "x😀y漢字é";
        for max in 0..=line.len() + 2 {
            let capped = cap_line_bytes(line, max.min(line.len()));
            assert!(line.starts_with(capped));
        }
    }
}
