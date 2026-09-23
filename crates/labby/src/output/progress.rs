//! Bounded progress feedback for long-running interactive CLI phases.

use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

use super::theme::{OutputFormat, SymbolMode};

/// A progress phase that is hidden for machine, redirected, plain, dumb, and CI output.
#[derive(Debug)]
pub struct ProgressPhase {
    bar: ProgressBar,
}

impl ProgressPhase {
    #[must_use]
    pub fn spinner(format: OutputFormat, message: impl Into<String>) -> Self {
        let context = format.render_context();
        let bar = if format.is_human() && context.animations_enabled() {
            ProgressBar::new_spinner()
        } else {
            ProgressBar::hidden()
        };
        const UNICODE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        const ASCII_FRAMES: &[&str] = &["-", "\\", "|", "/"];
        let frames = match context.symbols {
            SymbolMode::Unicode => UNICODE_FRAMES,
            SymbolMode::Ascii => ASCII_FRAMES,
        };
        let style = ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .expect("static progress template is valid")
            .tick_strings(frames);
        bar.set_style(style);
        bar.set_message(message.into());
        bar.enable_steady_tick(Duration::from_millis(100));
        Self { bar }
    }
}

impl Drop for ProgressPhase {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::theme::{ColorPolicy, RenderEnv};

    #[test]
    fn json_progress_is_hidden() {
        let format = OutputFormat::from_json_flag(
            true,
            ColorPolicy::Color,
            RenderEnv {
                stream_is_tty: true,
                no_color: false,
                term: Some("xterm-256color".into()),
                colorterm: None,
                lang: Some("en_US.UTF-8".into()),
                lab_symbols: None,
                columns: Some(80),
                ci: false,
            },
        );
        let progress = ProgressPhase::spinner(format, "Waiting");
        assert!(progress.bar.is_hidden());
    }

    #[test]
    fn ci_progress_is_hidden() {
        let format = OutputFormat::from_json_flag(
            false,
            ColorPolicy::Auto,
            RenderEnv {
                stream_is_tty: true,
                no_color: false,
                term: Some("xterm-256color".into()),
                colorterm: None,
                lang: Some("en_US.UTF-8".into()),
                lab_symbols: None,
                columns: Some(80),
                ci: true,
            },
        );
        let progress = ProgressPhase::spinner(format, "Waiting");
        assert!(progress.bar.is_hidden());
    }
}
