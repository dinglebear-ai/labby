module browser_request
open util/ordering[State]

abstract sig Phase {}
one sig Idle, Admitted, Dispatched, Terminal extends Phase {}

abstract sig Outcome {}
one sig Success, Error, Cancelled extends Outcome {}

sig State {
  phase: one Phase,
  outcome: lone Outcome,
  firstOutcome: lone Outcome,
  terminalWrites: one Int
}

pred typed[s: State] {
  s.terminalWrites >= 0
  (no s.outcome) iff s.phase != Terminal
  (no s.firstOutcome) iff no s.outcome
}

pred init[s: State] { s.phase = Idle and no s.outcome and no s.firstOutcome and s.terminalWrites = 0 }

pred nextStep[s, t: State] {
  (s.phase = Idle and t.phase = Admitted and no t.outcome and no t.firstOutcome and t.terminalWrites = s.terminalWrites)
  or (s.phase = Admitted and t.phase = Dispatched and no t.outcome and no t.firstOutcome and t.terminalWrites = s.terminalWrites)
  or (s.phase in Admitted + Dispatched and t.phase = Terminal and one t.outcome
      and t.firstOutcome = t.outcome and t.terminalWrites = add[s.terminalWrites, 1])
  or (s.phase = Terminal and t = s)
}

pred Lifecycle { (all s: State | typed[s]) and init[first] and (all s: State - last | nextStep[s, s.next]) }

assert SingleAuthoritativeTerminal {
  Lifecycle implies all s: State | s.terminalWrites <= 1
}

assert TerminalIsCleanAndImmutable {
  Lifecycle implies all s: State | (some s.outcome implies s.phase = Terminal) and s.outcome = s.firstOutcome
}

// Qualification controls: the first two checks must have no instance, while
// this predicate must produce one concrete overwrite-shaped instance.
pred BrokenOverwrite[s, t: State] {
  s.phase = Terminal
  t.phase = Terminal
  t.outcome != s.outcome
  t.firstOutcome = s.firstOutcome
  t.terminalWrites = add[s.terminalWrites, 1]
}

check SingleAuthoritativeTerminal for 4 but 4 Int
check TerminalIsCleanAndImmutable for 4 but 4 Int
run { (all s: State | typed[s]) and init[first] and (some s: State - last | BrokenOverwrite[s, s.next]) } for 5 but 5 Int
