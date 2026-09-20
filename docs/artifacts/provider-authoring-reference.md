---
title: Claude and Codex Artifact Authoring Reference
created: 2026-09-16
updated: 2026-09-16
---

# Claude and Codex Artifact Authoring Reference

This is the source-of-truth mapping used by Labby's Creator UI. It intentionally separates **portable Artifact metadata** from **provider-specific authoring formats**. Labby must not invent a universal frontmatter schema where Claude Code or Codex uses a different file type or sidecar.

Official sources checked on 2026-09-16:

- Claude Code skills: https://code.claude.com/docs/en/skills
- Claude Code subagents: https://code.claude.com/docs/en/sub-agents
- Claude Code plugins: https://code.claude.com/docs/en/plugins-reference
- Claude Code hooks: https://code.claude.com/docs/en/hooks
- Claude Code MCP: https://code.claude.com/docs/en/mcp
- Codex skills: https://developers.openai.com/codex/skills
- Codex subagents: https://developers.openai.com/codex/subagents
- Codex plugins: https://developers.openai.com/codex/plugins/build
- Codex custom prompts: https://developers.openai.com/codex/custom-prompts
- Codex hooks: https://developers.openai.com/codex/hooks
- Codex MCP: https://developers.openai.com/codex/mcp
- Codex configuration reference: https://developers.openai.com/codex/config-reference
- Codex App Server: https://developers.openai.com/codex/app-server.md

## Creator contract

1. Depot catalog tags are catalog metadata. They are not automatically emitted into Claude or Codex files.
2. Creator exposes only fields documented by the selected provider/artifact format.
3. Fields that are not frontmatter must stay in their native file. Examples: Codex subagents are TOML; Codex Skill presentation metadata belongs in `agents/openai.yaml`; plugin metadata belongs in `plugin.json`.
4. Portable Agent Skills fields remain available when they are part of the Agent Skills specification. Claude-only extensions are labeled **Claude** and Codex-only fields are labeled **Codex**.
5. A generated artifact must remain valid if Labby-specific catalog metadata is removed.

## Skills

### Portable Agent Skills / Codex SKILL.md baseline

Codex documents `SKILL.md` with required `name` and `description`. The portable Agent Skills vocabulary also supports the fields Labby already understands for interoperability:

- `name`
- `description`
- `license`
- `compatibility`
- `metadata`
- `allowed-tools`

Labby validates skill names as lowercase letters/digits separated by single hyphens, 1-64 characters, and keeps descriptions at or below the documented 1,024-character Agent Skills bound. Compatibility is bounded to 500 characters.

### Claude Code skill extensions

Claude Code additionally documents:

- `when_to_use`
- `argument-hint`
- `arguments`
- `disable-model-invocation`
- `user-invocable`
- `allowed-tools`
- `disallowed-tools`
- `model`
- `effort`
- `context`
- `agent`
- `background`
- `hooks`
- `paths`
- `shell`
- `metadata`

These are rendered by Creator only for Skills and are visibly marked Claude-specific where appropriate.

### Codex Skill presentation metadata

Codex does **not** put its presentation metadata in SKILL.md frontmatter. Optional UI/policy data is stored in `agents/openai.yaml`, including:

- `interface.display_name`
- `interface.short_description`
- `interface.icon_small`
- `interface.icon_large`
- `interface.brand_color`
- `interface.default_prompt`
- `policy.allow_implicit_invocation`
- tool dependency declarations

When Labby adds an OpenAI-sidecar editor, it must write that file separately rather than serializing these keys into SKILL.md.

## Agents / subagents

### Claude Code agents

Claude Code subagents are Markdown files with frontmatter. Documented fields include:

- `name`, `description`
- `tools`, `disallowedTools`
- `model`, `permissionMode`, `maxTurns`
- `skills`, `mcpServers`, `hooks`
- `memory`, `background`, `omitClaudeMd`, `effort`
- `isolation`, `color`, `initialPrompt`, `experimental`

### Codex agents

Codex custom agents are `.codex/agents/*.toml`, not Markdown frontmatter. The documented agent schema requires `name`, `description`, and `developer_instructions`, then accepts normal Codex configuration such as `model`, `model_reasoning_effort`, `sandbox_mode`, MCP server configuration, and skill configuration. Creator must not pretend the Claude Markdown schema applies to Codex agents.

## Commands and prompts

Claude Code's older `.claude/commands/*.md` command format is superseded by Skills for new work. Labby keeps legacy authoring fields needed for compatibility:

- `argument-hint`
- `allowed-tools`
- `model`

Codex custom prompts are also deprecated in favor of Skills. When authored, `~/.codex/prompts/*.md` uses a filename-derived name and documents:

- `description`
- `argument-hint`

Creator therefore omits `name:` from Codex Prompt frontmatter.

## Plugins

### Claude Code plugin manifest

Claude plugins use optional `.claude-plugin/plugin.json`; if the file exists, `name` is required. Documented root metadata includes `name`, `displayName`, `version`, `description`, `author`, `homepage`, `repository`, `license`, `keywords`, `metadata`, and `defaultEnabled`. Component declarations include Skills, commands, agents, hooks, MCP servers, output styles, and LSP servers, plus experimental/dependency data.

### Codex plugin manifest

Codex plugins use a portable root `plugin.json` with `$schema`, `name`, `version`, `description`, `author`, `homepage`, `repository`, `license`, and `keywords`. OpenAI-specific plugin presentation belongs below `extensions.com.openai`, including apps, hooks, interface metadata, developer/category/capability metadata, URLs, default prompt, brand color, icons/logos, and screenshots. Marketplace registration lives in `.agents/plugins/marketplace.json`, not artifact frontmatter.

## Hooks, MCP, Resources, Apps, Loadouts

These are body/manifest-native formats rather than a common Claude/Codex frontmatter format:

- Claude hooks use hook configuration documented by Claude Code.
- Codex hooks use its configured hook events and command handlers.
- Claude/Codex MCP server definitions belong in their documented MCP configuration.
- MCP Resources and MCP Apps are protocol assets/resources and metadata, not Claude/Codex Markdown frontmatter.
- Labby Loadouts are a Labby composition artifact. They must not be mislabeled as an upstream Claude/Codex format.

Creator shows a body-native message for these Artifact kinds until a dedicated structured editor exists.

## Regression requirements

Tests must prove at minimum that:

- Depot tags never leak into provider frontmatter.
- Codex Prompt names remain filename-derived.
- provider boolean/integer/JSON/list fields serialize to valid YAML values.
- Agent Skills naming and size constraints are enforced.
- provider-specific fields appear only on the applicable kind.
- new provider fields require an official documentation source before they are surfaced.
