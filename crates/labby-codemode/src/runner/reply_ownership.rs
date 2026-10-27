//! Native proof that every emitted request's reply was consumed before Done.

use std::collections::BTreeMap;

use crate::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput};

// This is a reuse-proof capacity, not a new dispatch or caller budget. If the
// proof cannot be retained, preserve execution semantics and retire the process.
const MAX_TRACKED_REPLIES: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReplyKind {
    Value,
    Snippet,
    StepDecision,
    StepRecorded,
}

#[derive(Default)]
pub(crate) struct ReplyOwnership {
    pending: BTreeMap<(u64, ReplyKind), usize>,
    count: usize,
    disqualified: bool,
}

impl ReplyOwnership {
    pub(crate) fn is_clean(&self) -> bool {
        !self.disqualified && self.count == 0
    }

    pub(crate) fn record_request(&mut self, output: &CodeModeRunnerOutput) {
        let (seq, kind) = match output {
            CodeModeRunnerOutput::ToolCall { seq, .. }
            | CodeModeRunnerOutput::ArtifactWrite { seq, .. } => (*seq, ReplyKind::Value),
            CodeModeRunnerOutput::SnippetResolve { seq, .. } => (*seq, ReplyKind::Snippet),
            CodeModeRunnerOutput::StepBegin { seq, .. } => (*seq, ReplyKind::StepDecision),
            CodeModeRunnerOutput::StepResult { seq, .. } => (*seq, ReplyKind::StepRecorded),
            CodeModeRunnerOutput::Done { .. } | CodeModeRunnerOutput::Error { .. } => return,
        };
        if self.disqualified {
            return;
        }
        if self.count == MAX_TRACKED_REPLIES {
            self.disqualify();
            return;
        }
        // Count multiplicity: caller-accessible native step emission can reuse
        // a seq, so a set alone could mistake one reply for two acknowledgments.
        *self.pending.entry((seq, kind)).or_default() += 1;
        self.count += 1;
    }

    pub(crate) fn record_reply(&mut self, input: &CodeModeRunnerInput) {
        if self.disqualified {
            return;
        }
        let key = match input {
            CodeModeRunnerInput::ToolResult { seq, .. } => Some((*seq, ReplyKind::Value)),
            CodeModeRunnerInput::SnippetResolved { seq, .. } => Some((*seq, ReplyKind::Snippet)),
            CodeModeRunnerInput::StepDecision { seq, .. } => Some((*seq, ReplyKind::StepDecision)),
            CodeModeRunnerInput::StepRecorded { seq } => Some((*seq, ReplyKind::StepRecorded)),
            // Every request kind can reject; one error acknowledges one request.
            CodeModeRunnerInput::ToolError { seq, .. } => self
                .pending
                .keys()
                .find(|(pending_seq, _)| pending_seq == seq)
                .copied(),
            CodeModeRunnerInput::Start { .. } => None,
        };
        if let Some(key) = key
            && let Some(count) = self.pending.get_mut(&key)
        {
            *count -= 1;
            if *count == 0 {
                self.pending.remove(&key);
            }
            self.count -= 1;
        } else {
            // Unexpected, duplicate and wrong-kind replies cannot establish a
            // clean boundary, even if caller JS ignores their settlement error.
            self.disqualify();
        }
    }

    fn disqualify(&mut self) {
        self.disqualified = true;
        self.pending.clear();
        self.count = 0;
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests;
