# Labby usage plugin

Read the repository root AGENTS.md. This package contains the canonical `.apm/skills/` sources for `install-labby`, `using-labby`, `using-codemode`, `using-snippets`, and `implement-in-microsandbox`. The separate `plugins/install-labby` Claude package owns guided installation and HTTP MCP client configuration. Do not add automatic lifecycle hooks or duplicate binary-owned setup behavior. Keep current action discovery and safe credential handling in the skills.

Regenerate the checked-in client packages with `python3 scripts/generate-native-plugins.py` after changing canonical skills or plugin metadata, then verify with `--check`. Preserve `AGENTS.md` as a regular file and its `CLAUDE.md` and `GEMINI.md` aliases.
