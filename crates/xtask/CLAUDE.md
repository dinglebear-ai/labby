# xtask — Repository Automation

`xtask` is repository-maintenance/build automation, not product runtime code.

Keep tasks deterministic, explicit about repository paths, and safe to run from supported developer/CI environments. Do not smuggle product behavior into xtask because it is convenient for a one-off migration.

When an xtask changes a generated artifact or build contract, update the owning product documentation and CI expectations in the same change.
