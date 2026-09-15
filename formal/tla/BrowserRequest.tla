------------------------- MODULE BrowserRequest -------------------------
EXTENDS Naturals

CONSTANT None

Phases == {None, "admitted", "dispatched", "terminal"}
Outcomes == {None, "success", "error", "cancelled"}

VARIABLES phase, outcome, firstOutcome, terminalWrites

vars == <<phase, outcome, firstOutcome, terminalWrites>>

Init ==
  /\ phase = None
  /\ outcome = None
  /\ firstOutcome = None
  /\ terminalWrites = 0

Admit ==
  /\ phase = None
  /\ outcome = None
  /\ phase' = "admitted"
  /\ UNCHANGED <<outcome, firstOutcome, terminalWrites>>

Dispatch ==
  /\ phase = "admitted"
  /\ phase' = "dispatched"
  /\ UNCHANGED <<outcome, firstOutcome, terminalWrites>>

Terminalize(value) ==
  /\ phase \in {"admitted", "dispatched"}
  /\ outcome = None
  /\ phase' = "terminal"
  /\ outcome' = value
  /\ firstOutcome' = value
  /\ terminalWrites' = terminalWrites + 1

Complete == phase = "dispatched" /\ Terminalize("success")
Fail == phase = "dispatched" /\ Terminalize("error")
Cancel == Terminalize("cancelled")

Next == Admit \/ Dispatch \/ Complete \/ Fail \/ Cancel \/ UNCHANGED vars

TypeOK ==
  /\ phase \in Phases
  /\ outcome \in Outcomes
  /\ firstOutcome \in Outcomes
  /\ terminalWrites \in Nat

SingleTerminal == terminalWrites <= 1
TerminalClean == outcome # None => phase = "terminal"
TerminalImmutable == outcome = firstOutcome

Spec == Init /\ [][Next]_vars
=============================================================================
