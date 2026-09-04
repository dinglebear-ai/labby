import type { EditorDiagnostic, EditorLanguage } from './types'

export const ARTIFACT_KINDS = ['Skill', 'Agent', 'Command', 'Prompt', 'MCP', 'Plugin', 'Hook', 'Loadout'] as const
export type ArtifactKind = (typeof ARTIFACT_KINDS)[number]

export interface ArtifactMetadata {
  name: string
  description: string
  license: string
  compatibility: string
  allowedTools: string
}

export interface ArtifactIssue extends EditorDiagnostic {
  field: keyof ArtifactMetadata | 'content'
}

const MARKDOWN_KINDS = new Set<ArtifactKind>(['Skill', 'Agent', 'Command', 'Prompt'])

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

export function composeArtifactSource(kind: ArtifactKind, metadata: ArtifactMetadata, content: string): string {
  if (!MARKDOWN_KINDS.has(kind)) return content
  const lines = ['---', `name: ${yamlScalar(metadata.name.trim())}`, `description: ${yamlScalar(metadata.description.trim())}`]
  if (metadata.license.trim()) lines.push(`license: ${yamlScalar(metadata.license.trim())}`)
  if (metadata.compatibility.trim()) lines.push(`compatibility: ${yamlScalar(metadata.compatibility.trim())}`)
  if (metadata.allowedTools.trim()) lines.push(`allowed-tools: ${yamlScalar(metadata.allowedTools.trim())}`)
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
    if (/^[\s]*\[/.test(metadata.allowedTools)) issues.push(issue('allowedTools', 'error', 'Allowed tools must be a space-separated string, not a YAML list.'))
  }

  if (metadata.name.includes('/') || metadata.name.includes('\\')) {
    issues.push(issue('name', 'error', 'Names cannot contain path separators.'))
  }

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

