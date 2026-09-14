---------------------- MODULE BrokenBrowserRequest ----------------------
EXTENDS BrowserRequest

Overwrite ==
  /\ phase = "terminal"
  /\ phase' = "terminal"
  /\ outcome' = IF outcome = "success" THEN "error" ELSE "success"
  /\ UNCHANGED firstOutcome
  /\ terminalWrites' = terminalWrites + 1

BrokenNext == Next \/ Overwrite
BrokenSpec == Init /\ [][BrokenNext]_vars
=============================================================================
