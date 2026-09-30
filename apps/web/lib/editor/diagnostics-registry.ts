import { getEditorDocumentConfig } from './language-registry'
import type { EditorAutocompleteItem, EditorDiagnostic } from './types'

export async function collectEditorDiagnostics(path: string, value: string): Promise<EditorDiagnostic[]> {
  const config = getEditorDocumentConfig(path)
  if (!config.validate) return []
  return await config.validate(path, value)
}

export async function collectEditorAutocomplete(path: string, value: string): Promise<EditorAutocompleteItem[]> {
  const config = getEditorDocumentConfig(path)
  if (!config.autocomplete) return []
  return await config.autocomplete(path, value)
}
