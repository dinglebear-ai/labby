# Session: gh-address-comments Skill Review and Script Referencing Fixes

**Date:** 2026-04-20  
**Branch:** fix/auth  
**Working Directory:** `/home/jmagar/workspace/lab`

---

## Session Overview

Reviewed and fixed the `gh-address-comments` Claude Code skill (`skills/gh-address-comments/SKILL.md`). The user reported the AI was not referencing the scripts correctly. A `plugin-dev:skill-reviewer` agent performed a systematic audit and identified 5 concrete issues. All were applied in this session.

---

## Timeline

1. Read `skills/gh-address-comments/SKILL.md` to understand skill structure
2. Discovered scripts are Python files in `skills/gh-address-comments/scripts/`, symlinked to `~/.local/bin/` as `gh-*` names
3. Checked `agents/openai.yaml` — found YAML syntax error
4. Read `scripts/ai_triage.py` — found undocumented `subprocess.run(["claude", "-p", ...])` behavior
5. Spawned `plugin-dev:skill-reviewer` agent for systematic audit
6. Applied all 5 fixes to SKILL.md, `agents/openai.yaml`, and `references/resolution-workflow.md`

---

## Key Findings

| # | Severity | Location | Finding |
|---|----------|----------|---------|
| 1 | Critical | `SKILL.md:25,79` | `gh-ai-triage` calls `subprocess.run(["claude", "-p", ...])` — completely undocumented; AI couldn't know it blocks on a nested Claude process |
| 2 | Critical | `SKILL.md:14-15` | "All scripts are in PATH" gave no tool invocation guidance; AI might try `python scripts/*.py` instead of using Bash tool |
| 3 | Major | `SKILL.md` workflow | Step 3 ("Track/Verify tracking setup") was missing entirely — workflow jumped from step 2 directly to step 4 |
| 4 | Minor | `agents/openai.yaml:3` | Unbalanced quote: `short_description: Address comments in a GitHub PR review"` |
| 5 | Minor | `references/resolution-workflow.md:29` | Referenced `scripts/fetch_comments.py` (source path) instead of `gh-fetch-comments` (symlink name) — inconsistent with skill's own naming |

---

## Technical Decisions

- **Callout block for `gh-ai-triage`** in both the table row and the workflow section: the table gives a one-liner warning; the workflow adds a `> Note:` blockquote before the bash example so it's impossible to miss at point of use.
- **Explicit Bash tool instruction** rather than just saying "in PATH": Claude Code has multiple ways to invoke Python (Edit, Write, Bash, inline) so "in PATH" was insufficient — the fix names the tool explicitly.
- **Restored step 3 as "Verify tracking setup"** rather than "Create beads" because bead creation is already automatic on fetch; the missing step was verification that beads exist, plus the recovery path (`gh-create-beads --dry-run`).

---

## Files Modified

| File | Change |
|------|--------|
| `skills/gh-address-comments/SKILL.md` | (1) Replaced "All scripts are in PATH" with Bash tool invocation instruction + no-source-file rule. (2) Added inline warning to `gh-ai-triage` table row. (3) Added `> Note:` callout block before `gh-ai-triage` workflow example. (4) Restored missing workflow step 3. |
| `skills/gh-address-comments/agents/openai.yaml` | Fixed unbalanced quote on `short_description` line. |
| `skills/gh-address-comments/references/resolution-workflow.md` | Changed `scripts/fetch_comments.py` → `gh-fetch-comments` on Phase 1 script reference. |

---

## Commands Executed

```bash
# Checked scripts in PATH
which gh-fetch-comments gh-pr-summary gh-create-beads gh-mark-resolved
# → all resolved to /home/jmagar/.local/bin/ symlinks

# Listed all gh-* symlinks and their targets
ls /home/jmagar/.local/bin/gh-*
# → 13 symlinks pointing to skills/gh-address-comments/scripts/*.py
```

---

## Behavior Changes (Before/After)

**Before:** An AI reading the skill would:
- Not know to use the Bash tool (vs Edit/inline Python)
- Potentially call `scripts/ai_triage.py` directly or expect JSON back from `gh-ai-triage`
- Not be warned that `gh-ai-triage` spawns a blocking nested `claude -p` subprocess
- Skip step 3 entirely (missing from workflow)
- Encounter a YAML parse error on `agents/openai.yaml`

**After:** An AI reading the skill will:
- Explicitly use the Bash tool with `gh-*` symlink names
- See inline and callout warnings that `gh-ai-triage` is a subprocess-Claude call returning plain text
- Follow a complete 8-step workflow including bead verification (step 3)
- Load `agents/openai.yaml` without YAML errors

---

## Verification Evidence

| Check | Expected | Actual | Status |
|-------|----------|--------|--------|
| Scripts in PATH | All 13 `gh-*` names resolve | All 13 confirmed via `which` and `ls` | ✅ Pass |
| SKILL.md step 3 exists | Workflow 1→2→3→4 sequence | Step 3 added | ✅ Pass |
| `gh-ai-triage` documented as subprocess | Warning in table + workflow | Both locations updated | ✅ Pass |
| Bash tool instruction present | Explicit "use Bash tool" text | Added at `Available CLI Tools` heading | ✅ Pass |
| `agents/openai.yaml` valid YAML | Balanced quotes | Quote fixed | ✅ Pass |
| `resolution-workflow.md` uses symlink name | `gh-fetch-comments` not `scripts/fetch_comments.py` | Fixed | ✅ Pass |

---

## Source IDs + Collections Touched

None — no vector search or embed/retrieve operations performed in this session.

---

## Risks and Rollback

- **Low risk:** All changes are documentation/skill content only — no code, no config, no binary changes.
- **Rollback:** `git checkout -- skills/gh-address-comments/` restores all three files.

---

## Decisions Not Taken

- **Full rewrite of SKILL.md** — rejected; the existing structure and content are sound, only targeted fixes were needed.
- **Moving `gh-ai-triage` callout to a separate "Prerequisites" section** — rejected; point-of-use placement is more effective than a preamble the AI might skip.
- **Trimming the `description` frontmatter** (reviewer flagged it at ~750 chars, suggested cap ~500) — deferred; the extra trigger phrases are low-harm and the user didn't ask for frontmatter changes.

---

## Open Questions

- Is the `agents/openai.yaml` file actually used by any loader, or is it vestigial? Its current content is only display metadata with no agent instructions — a follow-up could add proper agent instructions if the format is live.
- Are there other `references/*.md` files that also reference `scripts/*.py` source paths instead of `gh-*` symlink names?

---

## Next Steps

- Optionally: scan remaining `references/*.md` files for any other `scripts/*.py` direct references.
- Optionally: expand `agents/openai.yaml` if the interface loader accepts richer agent instructions.
