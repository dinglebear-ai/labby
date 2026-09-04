export type EditorLanguage =
  | 'json'
  | 'yaml'
  | 'markdown'
  | 'bash'
  | 'toml'
  | 'dotenv'
  | 'javascript'
  | 'typescript'
  | 'text'

export type EditorDiagnosticSeverity = 'error' | 'warning' | 'info'

export interface EditorDiagnostic {
  from: number
  to: number
  severity: EditorDiagnosticSeverity
  message: string
}

export interface EditorAutocompleteItem {
  label: string
  type?: string
  detail?: string
  apply?: string
}

export type EditorValidator = (path: string, value: string) => EditorDiagnostic[] | Promise<EditorDiagnostic[]>
export type EditorAutocompleteProvider = (path: string, value: string) => EditorAutocompleteItem[] | Promise<EditorAutocompleteItem[]>

export interface EditorDocumentConfig {
  language: EditorLanguage
  validate?: EditorValidator
  autocomplete?: EditorAutocompleteProvider
}

export interface MarketplaceWorkspaceFile {
  path: string
  lang: EditorLanguage
  content: string
  savedContent?: string
  dirty?: boolean
}

export interface PluginWorkspace {
  pluginId: string
  files: MarketplaceWorkspaceFile[]
  hasDirtyFiles?: boolean
  deployTarget?: string
}

export interface SavePluginWorkspaceFileInput {
  pluginId: string
  path: string
  content: string
}

export interface SavePluginWorkspaceFileResult {
  savedAt: string
}

export interface DeployPluginWorkspaceResult {
  ok: boolean
  changed: string[]
  skipped: string[]
  removed: string[]
  failed: string[]
  target?: string
}

export interface DeployPluginWorkspacePreviewResult {
  changed: string[]
  skipped: string[]
  removed: string[]
  entries?: DeployPluginWorkspacePreviewEntry[]
  target?: string
}

export interface DeployPluginWorkspacePreviewEntry {
  path: string
  status: 'changed' | 'removed'
  beforeContent?: string | null
  afterContent?: string | null
}
