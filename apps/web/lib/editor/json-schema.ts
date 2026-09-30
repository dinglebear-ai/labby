import type { EditorAutocompleteItem, EditorDiagnostic } from './types'

interface JsonSchemaField {
  key: string
  valueSnippet: string
  detail: string
  required?: boolean
}

const SCHEMAS: Record<string, JsonSchemaField[]> = {
  'plugin.json': [
    { key: 'name', valueSnippet: '"plugin-name"', detail: 'Plugin name', required: true },
    { key: 'version', valueSnippet: '"1.0.0"', detail: 'Plugin version', required: true },
    { key: 'description', valueSnippet: '"Plugin description"', detail: 'Plugin description' },
    { key: 'category', valueSnippet: '"workflow"', detail: 'Plugin category' },
    { key: 'tags', valueSnippet: '["workflow"]', detail: 'Tags list' },
  ],
}

function schemaForPath(path: string): JsonSchemaField[] {
  const normalized = path.replaceAll('\\', '/')
  const basename = normalized.split('/').pop() ?? normalized
  return SCHEMAS[basename] ?? []
}

export function validateJsonDocument(path: string, value: string): EditorDiagnostic[] {
  let parsed: unknown
  try {
    parsed = JSON.parse(value)
  } catch (error) {
    return [{
      from: 0,
      to: value.length,
      severity: 'error',
      message: error instanceof Error ? error.message : 'Invalid JSON',
    }]
  }

  const fields = schemaForPath(path)
  if (!fields.length || typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return []
  }

  const diagnostics: EditorDiagnostic[] = []
  const record = parsed as Record<string, unknown>
  for (const field of fields) {
    if (field.required && !(field.key in record)) {
      diagnostics.push({
        from: 0,
        to: value.length,
        severity: 'warning',
        message: `Recommended key missing: \`${field.key}\``,
      })
    }
  }
  return diagnostics
}

export function autocompleteJsonDocument(path: string): EditorAutocompleteItem[] {
  return schemaForPath(path).map((field) => ({
    label: field.key,
    type: 'property',
    detail: field.detail,
    apply: `"${field.key}": ${field.valueSnippet}`,
  }))
}
