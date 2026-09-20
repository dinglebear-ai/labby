import type { EditorDiagnostic, EditorLanguage } from './types'

export const ARTIFACT_KINDS = ['Skill', 'Agent', 'Command', 'Prompt', 'MCP', 'Plugin', 'Hook', 'Loadout'] as const
export type ArtifactKind = (typeof ARTIFACT_KINDS)[number]

export type ArtifactFieldFormat = 'string' | 'boolean' | 'integer' | 'list' | 'json'
export type ArtifactEcosystem = 'Portable' | 'Claude' | 'Codex'

export interface ArtifactFrontmatterFieldSpec {
  key: string
  label: string
  frontmatterKey: string
  description: string
  ecosystem: ArtifactEcosystem
  format?: ArtifactFieldFormat
  placeholder?: string
}

export interface ArtifactMetadata {
  name: string
  description: string
  /** Depot catalog tags. Provider artifact files do not inherit these automatically. */
  tags: string[]
  /** Portable Agent Skills fields kept for backwards-compatible drafts/WebMCP. */
  license: string
  compatibility: string
  allowedTools: string
  /** Provider-specific fields keyed by their canonical frontmatter name. */
  frontmatter?: Record<string, string>
}

export interface ArtifactIssue extends EditorDiagnostic {
  field: keyof ArtifactMetadata | 'content' | 'frontmatter'
}

const MARKDOWN_KINDS = new Set<ArtifactKind>(['Skill', 'Agent', 'Command', 'Prompt'])

const SKILL_FIELDS: ArtifactFrontmatterFieldSpec[] = [
  { key: 'license', label: 'License', frontmatterKey: 'license', ecosystem: 'Portable', description: 'License metadata from the Agent Skills specification.' },
  { key: 'compatibility', label: 'Compatibility', frontmatterKey: 'compatibility', ecosystem: 'Portable', description: 'Environment or product compatibility notes. Agent Skills limits this to 500 characters.' },
  { key: 'allowed-tools', label: 'Allowed tools', frontmatterKey: 'allowed-tools', ecosystem: 'Portable', description: 'Tools pre-approved for the skill.' },
  { key: 'metadata', label: 'Metadata', frontmatterKey: 'metadata', ecosystem: 'Portable', format: 'json', placeholder: '{"team":"platform"}', description: 'Free-form metadata map. Enter a JSON object.' },
  { key: 'when_to_use', label: 'When to use', frontmatterKey: 'when_to_use', ecosystem: 'Claude', description: 'Additional Claude Code invocation guidance appended to the description.' },
  { key: 'argument-hint', label: 'Argument hint', frontmatterKey: 'argument-hint', ecosystem: 'Claude', description: 'Autocomplete hint for expected skill arguments.' },
  { key: 'arguments', label: 'Arguments', frontmatterKey: 'arguments', ecosystem: 'Claude', description: 'Named positional arguments used by Claude Code string substitution.' },
  { key: 'disable-model-invocation', label: 'Manual only', frontmatterKey: 'disable-model-invocation', ecosystem: 'Claude', format: 'boolean', placeholder: 'true', description: 'Prevent Claude from automatically invoking this skill.' },
  { key: 'user-invocable', label: 'User invocable', frontmatterKey: 'user-invocable', ecosystem: 'Claude', format: 'boolean', placeholder: 'true', description: 'Whether the skill appears in Claude Code slash-command surfaces.' },
  { key: 'disallowed-tools', label: 'Disallowed tools', frontmatterKey: 'disallowed-tools', ecosystem: 'Claude', description: 'Tools removed from Claude while the skill is active.' },
  { key: 'model', label: 'Model', frontmatterKey: 'model', ecosystem: 'Claude', description: 'Claude model override for the current turn, or inherit.' },
  { key: 'effort', label: 'Effort', frontmatterKey: 'effort', ecosystem: 'Claude', description: 'Claude effort override: low, medium, high, xhigh, or max.' },
  { key: 'context', label: 'Context', frontmatterKey: 'context', ecosystem: 'Claude', placeholder: 'fork', description: 'Set to fork to run the skill in a forked subagent context.' },
  { key: 'agent', label: 'Agent', frontmatterKey: 'agent', ecosystem: 'Claude', description: 'Subagent type to use when context is fork.' },
  { key: 'background', label: 'Background', frontmatterKey: 'background', ecosystem: 'Claude', format: 'boolean', description: 'Whether a forked skill runs in the background.' },
  { key: 'hooks', label: 'Hooks', frontmatterKey: 'hooks', ecosystem: 'Claude', format: 'json', placeholder: '{"PreToolUse":[]}', description: 'Skill-scoped lifecycle hooks. Enter the hooks map as JSON.' },
  { key: 'paths', label: 'Paths', frontmatterKey: 'paths', ecosystem: 'Claude', description: 'Glob patterns that constrain automatic activation.' },
  { key: 'shell', label: 'Shell', frontmatterKey: 'shell', ecosystem: 'Claude', placeholder: 'bash', description: 'Shell for dynamic context commands: bash or powershell.' },
]

const AGENT_FIELDS: ArtifactFrontmatterFieldSpec[] = [
  { key: 'tools', label: 'Tools', frontmatterKey: 'tools', ecosystem: 'Claude', description: 'Tools available to the Claude subagent.' },
  { key: 'disallowedTools', label: 'Disallowed tools', frontmatterKey: 'disallowedTools', ecosystem: 'Claude', description: 'Tools removed from inherited or explicit tools.' },
  { key: 'model', label: 'Model', frontmatterKey: 'model', ecosystem: 'Claude', description: 'sonnet, opus, haiku, fable, a full model ID, or inherit.' },
  { key: 'permissionMode', label: 'Permission mode', frontmatterKey: 'permissionMode', ecosystem: 'Claude', description: 'default, acceptEdits, auto, dontAsk, bypassPermissions, plan, or manual.' },
  { key: 'maxTurns', label: 'Max turns', frontmatterKey: 'maxTurns', ecosystem: 'Claude', format: 'integer', description: 'Maximum number of agentic turns.' },
  { key: 'skills', label: 'Preloaded skills', frontmatterKey: 'skills', ecosystem: 'Claude', format: 'list', description: 'Skills injected into the subagent context at startup.' },
  { key: 'mcpServers', label: 'MCP servers', frontmatterKey: 'mcpServers', ecosystem: 'Claude', format: 'json', placeholder: '["github"]', description: 'Server names or inline MCP definitions. Enter JSON.' },
  { key: 'hooks', label: 'Hooks', frontmatterKey: 'hooks', ecosystem: 'Claude', format: 'json', description: 'Lifecycle hooks scoped to this subagent. Enter JSON.' },
  { key: 'memory', label: 'Memory', frontmatterKey: 'memory', ecosystem: 'Claude', description: 'Persistent memory scope: user, project, or local.' },
  { key: 'background', label: 'Background', frontmatterKey: 'background', ecosystem: 'Claude', format: 'boolean', description: 'Keep this subagent in the background.' },
  { key: 'omitClaudeMd', label: 'Omit CLAUDE.md', frontmatterKey: 'omitClaudeMd', ecosystem: 'Claude', format: 'boolean', description: 'Launch without user/project/local CLAUDE.md files.' },
  { key: 'effort', label: 'Effort', frontmatterKey: 'effort', ecosystem: 'Claude', description: 'Effort override: low, medium, high, xhigh, or max.' },
  { key: 'isolation', label: 'Isolation', frontmatterKey: 'isolation', ecosystem: 'Claude', placeholder: 'worktree', description: 'Set to worktree for a temporary isolated git worktree.' },
  { key: 'color', label: 'Color', frontmatterKey: 'color', ecosystem: 'Claude', description: 'red, blue, green, yellow, purple, orange, pink, or cyan.' },
  { key: 'initialPrompt', label: 'Initial prompt', frontmatterKey: 'initialPrompt', ecosystem: 'Claude', description: 'First user turn when this agent runs as the main session agent.' },
  { key: 'experimental', label: 'Experimental', frontmatterKey: 'experimental', ecosystem: 'Claude', format: 'json', placeholder: '{"cacheTtl":"1h"}', description: 'Experimental agent options such as cacheTtl. Enter JSON.' },
]

const COMMAND_FIELDS: ArtifactFrontmatterFieldSpec[] = [
  { key: 'argument-hint', label: 'Argument hint', frontmatterKey: 'argument-hint', ecosystem: 'Claude', description: 'Autocomplete hint for legacy .claude/commands files. Claude recommends skills for new commands.' },
  { key: 'allowed-tools', label: 'Allowed tools', frontmatterKey: 'allowed-tools', ecosystem: 'Claude', description: 'Tools pre-approved while the command runs.' },
  { key: 'model', label: 'Model', frontmatterKey: 'model', ecosystem: 'Claude', description: 'Model override while this command executes.' },
]

const PROMPT_FIELDS: ArtifactFrontmatterFieldSpec[] = [
  { key: 'argument-hint', label: 'Argument hint', frontmatterKey: 'argument-hint', ecosystem: 'Codex', description: 'Expected KEY=value or positional arguments shown for a Codex custom prompt.' },
]

export function artifactFrontmatterFields(kind: ArtifactKind): readonly ArtifactFrontmatterFieldSpec[] {
  if (kind === 'Skill') return SKILL_FIELDS
  if (kind === 'Agent') return AGENT_FIELDS
  if (kind === 'Command') return COMMAND_FIELDS
  if (kind === 'Prompt') return PROMPT_FIELDS
  return []
}

export function artifactFrontmatterValue(metadata: ArtifactMetadata, key: string): string {
  const direct = metadata.frontmatter?.[key]
  if (direct !== undefined) return direct
  if (key === 'license') return metadata.license
  if (key === 'compatibility') return metadata.compatibility
  if (key === 'allowed-tools') return metadata.allowedTools
  return ''
}

export function artifactPath(kind: ArtifactKind, name: string): string {
  const safeName = name.trim() || 'untitled'
  if (kind === 'Skill') return `skills/${safeName}/SKILL.md`
  if (kind === 'Agent') return `agents/${safeName}.md`
  if (kind === 'Command') return `commands/${safeName}.md`
  if (kind === 'Prompt') return `prompts/${safeName}.md`
  if (kind === 'Hook') return `hooks/${safeName}.sh`
  return `${kind.toLowerCase()}s/${safeName}.json`
}

export function artifactLanguage(kind: ArtifactKind): EditorLanguage {
  if (MARKDOWN_KINDS.has(kind)) return 'markdown'
  if (kind === 'Hook') return 'bash'
  return 'json'
}

function yamlScalar(value: string): string {
  return JSON.stringify(value)
}

/** Trims, strips a leading `#`, drops empties, and dedupes while preserving first-seen order. */
export function normalizeArtifactTags(tags: readonly string[]): string[] {
  return Array.from(new Set(tags.map(tag => tag.trim().replace(/^#/, '')).filter(Boolean)))
}

export const ARTIFACT_TAG_LIMIT = 64
export const ARTIFACT_TAG_MAX_BYTES = 64

function legacyFrontmatter(metadata: ArtifactMetadata): Record<string, string> {
  return {
    ...(metadata.license.trim() ? { license: metadata.license } : {}),
    ...(metadata.compatibility.trim() ? { compatibility: metadata.compatibility } : {}),
    ...(metadata.allowedTools.trim() ? { 'allowed-tools': metadata.allowedTools } : {}),
  }
}

function serializeFrontmatterValue(value: string, format: ArtifactFieldFormat = 'string'): string {
  const trimmed = value.trim()
  if (format === 'boolean') return /^(true|yes|on|1)$/i.test(trimmed) ? 'true' : /^(false|no|off|0)$/i.test(trimmed) ? 'false' : yamlScalar(trimmed)
  if (format === 'integer' && /^\d+$/.test(trimmed)) return trimmed
  if (format === 'list') {
    if (trimmed.startsWith('[')) {
      try { const parsed = JSON.parse(trimmed); if (Array.isArray(parsed)) return JSON.stringify(parsed) } catch { /* fall through */ }
    }
    return JSON.stringify(trimmed.split(/[\s,]+/).filter(Boolean))
  }
  if (format === 'json') {
    try {
      const parsed = JSON.parse(trimmed)
      if (parsed && typeof parsed === 'object') return JSON.stringify(parsed)
    } catch { /* Invalid drafts stay editable; validation reports the malformed value. */ }
  }
  return yamlScalar(trimmed)
}

export function composeArtifactSource(kind: ArtifactKind, metadata: ArtifactMetadata, content: string): string {
  if (!MARKDOWN_KINDS.has(kind)) return content
  const lines = ['---']
  // Codex custom prompt names are filename-derived. Claude/Agent Skills use an explicit name.
  if (kind !== 'Prompt') lines.push(`name: ${yamlScalar(metadata.name.trim())}`)
  lines.push(`description: ${yamlScalar(metadata.description.trim())}`)
  const values = { ...legacyFrontmatter(metadata), ...(metadata.frontmatter ?? {}) }
  for (const field of artifactFrontmatterFields(kind)) {
    const value = values[field.frontmatterKey]?.trim()
    if (!value) continue
    lines.push(`${field.frontmatterKey}: ${serializeFrontmatterValue(value, field.format)}`)
  }
  lines.push('---', '', content)
  return lines.join('\n')
}

function issue(field: ArtifactIssue['field'], severity: ArtifactIssue['severity'], message: string, content = ''): ArtifactIssue {
  return { field, severity, message, from: 0, to: Math.min(1, content.length) }
}

export function validateArtifactDraft(kind: ArtifactKind, metadata: ArtifactMetadata, content: string): ArtifactIssue[] {
  const issues: ArtifactIssue[] = []
  const name = metadata.name.trim()
  const description = metadata.description.trim()

  if (!name) issues.push(issue('name', 'error', 'Name is required.'))
  if (!description && MARKDOWN_KINDS.has(kind)) issues.push(issue('description', 'error', 'Description is required.'))

  if (kind === 'Skill') {
    if (name.length > 64 || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) {
      issues.push(issue('name', 'error', 'Agent Skills names use 1–64 lowercase letters, digits, and single hyphens.'))
    }
    if (description.length > 1024) issues.push(issue('description', 'error', 'Agent Skills descriptions cannot exceed 1,024 characters.'))
    if (metadata.compatibility.length > 500) issues.push(issue('compatibility', 'error', 'Compatibility cannot exceed 500 characters.'))
  }

  if (kind === 'Agent' && !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) {
    issues.push(issue('name', 'error', 'Claude subagent names use lowercase letters, digits, and single hyphens.'))
  }

  const frontmatter = { ...legacyFrontmatter(metadata), ...(metadata.frontmatter ?? {}) }
  for (const field of artifactFrontmatterFields(kind)) {
    const value = frontmatter[field.frontmatterKey]?.trim()
    if (!value) continue
    if (field.format === 'boolean' && !/^(true|false|yes|no|on|off|1|0)$/i.test(value)) {
      issues.push(issue('frontmatter', 'error', `${field.label} must be a boolean value.`))
    } else if (field.format === 'integer' && !/^\d+$/.test(value)) {
      issues.push(issue('frontmatter', 'error', `${field.label} must be a non-negative integer.`))
    } else if (field.format === 'json') {
      try {
        const parsed = JSON.parse(value)
        if (!parsed || typeof parsed !== 'object') issues.push(issue('frontmatter', 'error', `${field.label} must be a JSON object or array.`))
      } catch {
        issues.push(issue('frontmatter', 'error', `${field.label} must be valid JSON.`))
      }
    }
  }

  if (metadata.name.includes('/') || metadata.name.includes('\\')) {
    issues.push(issue('name', 'error', 'Names cannot contain path separators.'))
  }

  // Depot bounds descriptor tags to 64 entries of 1–64 UTF-8 bytes; surface that before publishing.
  const tags = normalizeArtifactTags(metadata.tags)
  if (tags.length > ARTIFACT_TAG_LIMIT) issues.push(issue('tags', 'error', `Use at most ${ARTIFACT_TAG_LIMIT} tags.`))
  if (tags.some(tag => new TextEncoder().encode(tag).length > ARTIFACT_TAG_MAX_BYTES)) issues.push(issue('tags', 'error', `Tags cannot exceed ${ARTIFACT_TAG_MAX_BYTES} bytes each.`))

  if (!content.trim()) {
    issues.push(issue('content', 'error', `${kind} content cannot be empty.`, content))
  } else if (MARKDOWN_KINDS.has(kind)) {
    if (!/^#{1,3}\s+\S/m.test(content)) issues.push(issue('content', 'warning', 'Add at least one heading so readers can scan this artifact.', content))
    if (/\b(TODO|TBD|FIXME)\b/i.test(content)) issues.push(issue('content', 'warning', 'Resolve placeholder text before publishing.', content))
  } else if (artifactLanguage(kind) === 'json') {
    try {
      const parsed = JSON.parse(content) as unknown
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) issues.push(issue('content', 'error', `${kind} JSON must be an object.`, content))
    } catch (error) {
      issues.push(issue('content', 'error', `Invalid JSON: ${error instanceof Error ? error.message : 'unable to parse document'}`, content))
    }
  } else if (kind === 'Hook' && !content.startsWith('#!')) {
    issues.push(issue('content', 'warning', 'Hooks should begin with a portable shebang.', content))
  }

  return issues
}

