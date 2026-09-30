import type { EditorAutocompleteItem, EditorDiagnostic } from './types'

interface KnownTomlSchema {
  paths: string[]
  autocomplete: EditorAutocompleteItem[]
  requiredRootKeys?: string[]
  themeValues?: string[]
}

const KNOWN_TOML_SCHEMAS: KnownTomlSchema[] = [
  {
    paths: ['labby.toml'],
    autocomplete: [
      { label: 'title', type: 'property', detail: 'Display title', apply: 'title = "Labby"' },
      { label: 'description', type: 'property', detail: 'Short workspace description', apply: 'description = "Local Claude workspace mirror"' },
      { label: 'theme', type: 'property', detail: 'Preferred theme', apply: 'theme = "aurora"' },
      { label: '[deploy]', type: 'keyword', detail: 'Deployment settings table', apply: '[deploy]\ntarget = "local"' },
      { label: 'target', type: 'property', detail: 'Deployment target profile', apply: 'target = "local"' },
    ],
    requiredRootKeys: ['title', 'theme'],
    themeValues: ['aurora', 'light', 'dark', 'system'],
  },
]

const DEFAULT_TOML_AUTOCOMPLETE: EditorAutocompleteItem[] = [
  { label: 'title', type: 'property', detail: 'Display title', apply: 'title = "Labby"' },
]

function resolveTomlSchema(path: string): KnownTomlSchema | null {
  const normalized = path.replaceAll('\\', '/')
  const basename = normalized.split('/').pop() ?? normalized
  return KNOWN_TOML_SCHEMAS.find((schema) => schema.paths.includes(normalized) || schema.paths.includes(basename)) ?? null
}

function looksInvalidToml(value: string): string | null {
  if (!value.trim()) return null
  for (const line of value.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed || trimmed.startsWith('#') || (trimmed.startsWith('[') && trimmed.endsWith(']'))) {
      continue
    }
    if (trimmed.startsWith('[') && !trimmed.endsWith(']')) {
      return 'Unclosed TOML table header'
    }
    if (trimmed.includes('=') && /=$/.test(trimmed)) {
      return 'Expected a value after `=`'
    }
  }
  return null
}

function collectTomlAssignments(value: string): Record<string, string> {
  const assignments: Record<string, string> = {}
  let currentTable = ''

  for (const rawLine of value.split('\n')) {
    const line = rawLine.trim()
    if (!line || line.startsWith('#')) continue
    if (line.startsWith('[') && line.endsWith(']')) {
      currentTable = line.slice(1, -1).trim()
      continue
    }
    const eqIndex = line.indexOf('=')
    if (eqIndex <= 0) continue
    const key = line.slice(0, eqIndex).trim()
    const rhs = line.slice(eqIndex + 1).trim().replace(/^"(.*)"$/, '$1')
    const qualifiedKey = currentTable ? `${currentTable}.${key}` : key
    assignments[qualifiedKey] = rhs
  }

  return assignments
}

export function validateTomlDocument(path: string, value: string): EditorDiagnostic[] {
  const issue = looksInvalidToml(value)
  if (issue) {
    return [{ from: 0, to: value.length, severity: 'error', message: issue }]
  }

  const diagnostics: EditorDiagnostic[] = []
  const assignments = collectTomlAssignments(value)
  const schema = resolveTomlSchema(path)

  for (const key of schema?.requiredRootKeys ?? []) {
    if (!assignments[key]) {
      diagnostics.push({
        from: 0,
        to: value.length,
        severity: 'warning',
        message: `Missing recommended key \`${key}\``,
      })
    }
  }

  if (schema?.themeValues && assignments.theme && !new Set(schema.themeValues).has(assignments.theme)) {
    diagnostics.push({
      from: 0,
      to: value.length,
      severity: 'error',
      message: `Unsupported theme \`${assignments.theme}\``,
    })
  }

  if (assignments['deploy.target'] && assignments['deploy.target'] !== 'local') {
    diagnostics.push({
      from: 0,
      to: value.length,
      severity: 'info',
      message: 'Only `local` deployment is supported in the current rollout',
    })
  }

  return diagnostics
}

export function autocompleteTomlDocument(path: string): EditorAutocompleteItem[] {
  return resolveTomlSchema(path)?.autocomplete ?? DEFAULT_TOML_AUTOCOMPLETE
}
