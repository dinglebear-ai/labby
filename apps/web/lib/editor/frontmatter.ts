import type { EditorDiagnostic } from './types'

const REQUIRED_BY_CATEGORY: Record<string, string[]> = {
  agents: ['name', 'description'],
  skills: ['name', 'description'],
  commands: ['name'],
}

function detectCategory(path: string): keyof typeof REQUIRED_BY_CATEGORY | null {
  const normalized = path.replaceAll('\\', '/')
  if (normalized.startsWith('agents/')) return 'agents'
  if (normalized.startsWith('skills/')) return 'skills'
  if (normalized.startsWith('commands/')) return 'commands'
  return null
}

function parseFrontmatter(value: string): { attributes: Record<string, string>; bodyStart: number } | null {
  if (!value.startsWith('---\n')) return null
  const end = value.indexOf('\n---', 4)
  if (end === -1) return null
  const raw = value.slice(4, end)
  const attributes: Record<string, string> = {}
  for (const line of raw.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed) continue
    const split = trimmed.indexOf(':')
    if (split === -1) continue
    const key = trimmed.slice(0, split).trim()
    const val = trimmed.slice(split + 1).trim()
    attributes[key] = val
  }
  return { attributes, bodyStart: end + 4 }
}

export function validateClaudeFrontmatter(path: string, value: string): EditorDiagnostic[] {
  const category = detectCategory(path)
  if (!category) return []

  const parsed = parseFrontmatter(value)
  if (!parsed) {
    return [{ from: 0, to: 0, severity: 'error', message: 'Expected YAML frontmatter block at the start of the file' }]
  }

  const diagnostics: EditorDiagnostic[] = []
  for (const field of REQUIRED_BY_CATEGORY[category]) {
    if (!parsed.attributes[field]) {
      diagnostics.push({
        from: 0,
        to: parsed.bodyStart,
        severity: 'error',
        message: `Missing required frontmatter field \`${field}\``,
      })
    }
  }

  return diagnostics
}
