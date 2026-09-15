//! Finite standalone Kani model for browser request terminal authority.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Admitted,
    Dispatched,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Success,
    Error,
    CancelledBeforeDispatch,
    CancelledAfterDispatch,
}

#[derive(Clone, Copy)]
enum Action {
    Admit,
    Dispatch,
    CompleteSuccess,
    CompleteError,
    Cancel,
}

#[derive(Clone, Copy, Default)]
struct Request {
    active: Option<Phase>,
    terminal: Option<Terminal>,
    first_terminal: Option<Terminal>,
    terminal_writes: u8,
}

impl Request {
    fn terminalize(&mut self, terminal: Terminal) {
        if self.active.is_none() || self.terminal.is_some() {
            return;
        }
        self.active = None;
        self.terminal = Some(terminal);
        self.first_terminal = Some(terminal);
        self.terminal_writes += 1;
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Admit if self.active.is_none() && self.terminal.is_none() => {
                self.active = Some(Phase::Admitted);
            }
            Action::Dispatch if self.active == Some(Phase::Admitted) => {
                self.active = Some(Phase::Dispatched);
            }
            Action::CompleteSuccess if self.active == Some(Phase::Dispatched) => {
                self.terminalize(Terminal::Success);
            }
            Action::CompleteError if self.active == Some(Phase::Dispatched) => {
                self.terminalize(Terminal::Error);
            }
            Action::Cancel => match self.active {
                Some(Phase::Admitted) => self.terminalize(Terminal::CancelledBeforeDispatch),
                Some(Phase::Dispatched) => self.terminalize(Terminal::CancelledAfterDispatch),
                None => {}
            },
            _ => {}
        }
    }

    fn assert_terminal_authority(&self) {
        assert!(self.terminal_writes <= 1);
        assert!(self.terminal.is_none() || self.active.is_none());
        assert!(self.first_terminal == self.terminal);
    }
}

fn arbitrary_action() -> Action {
    match kani::any::<u8>() % 5 {
        0 => Action::Admit,
        1 => Action::Dispatch,
        2 => Action::CompleteSuccess,
        3 => Action::CompleteError,
        _ => Action::Cancel,
    }
}

/// Proves terminal ownership across all finite action sequences of length four.
#[kani::proof]
fn request_single_terminal() {
    let mut request = Request::default();
    for _ in 0..4 {
        request.apply(arbitrary_action());
        request.assert_terminal_authority();
    }
}

/// Negative qualification control: demonstrates the checker detects overwrite.
#[kani::proof]
fn request_single_terminal_negative() {
    let mut request = Request::default();
    request.apply(Action::Admit);
    request.apply(Action::Dispatch);
    request.apply(Action::CompleteSuccess);

    // This is the historical failure shape: a late result overwrites an already
    // authoritative terminal observation and records a second terminal write.
    request.terminal = Some(Terminal::Error);
    request.terminal_writes += 1;
    request.assert_terminal_authority();
}
