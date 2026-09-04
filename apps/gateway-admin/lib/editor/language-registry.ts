import { markdown } from '@codemirror/lang-markdown'
import { json } from '@codemirror/lang-json'
import { javascript } from '@codemirror/lang-javascript'
import { yaml } from '@codemirror/lang-yaml'
import { StreamLanguage } from '@codemirror/language'
import { shell } from '@codemirror/legacy-modes/mode/shell'
import { toml } from '@codemirror/legacy-modes/mode/toml'
import { properties } from '@codemirror/legacy-modes/mode/properties'
import type { Extension } from '@codemirror/state'

import { autocompleteJsonDocument, validateJsonDocument } from './json-schema'
import { autocompleteTomlDocument, validateTomlDocument } from './toml-schema'
import { validateClaudeFrontmatter } from './frontmatter'
import type { EditorDocumentConfig, EditorLanguage } from './types'

export function detectEditorLanguage(path: string): EditorLanguage {
  const normalized = path.replaceAll('\\', '/')
  if (normalized.endsWith('/.env') || normalized === '.env' || /\.env\.[^/]+$/.test(normalized)) return 'dotenv'
  if (normalized.endsWith('.json')) return 'json'
  if (normalized.endsWith('.yaml') || normalized.endsWith('.yml')) return 'yaml'
  if (normalized.endsWith('.md')) return 'markdown'
  if (normalized.endsWith('.sh') || normalized.endsWith('.bash')) return 'bash'
  if (normalized.endsWith('.toml')) return 'toml'
  if (normalized.endsWith('.ts') || normalized.endsWith('.tsx')) return 'typescript'
  if (normalized.endsWith('.js') || normalized.endsWith('.jsx')) return 'javascript'
  return 'text'
}

export function getEditorDocumentConfig(path: string): EditorDocumentConfig {
  const language = detectEditorLanguage(path)
  if (language === 'json') {
    return {
      language,
      validate: validateJsonDocument,
      autocomplete: (targetPath) => autocompleteJsonDocument(targetPath),
    }
  }
  if (language === 'toml') {
    return {
      language,
      validate: validateTomlDocument,
      autocomplete: (targetPath) => autocompleteTomlDocument(targetPath),
    }
  }
  if (language === 'markdown' && /^(agents|skills|commands)\//.test(path.replaceAll('\\', '/'))) {
    return {
      language,
      validate: validateClaudeFrontmatter,
    }
  }
  return { language }
}

export async function loadLanguageExtension(language: EditorLanguage): Promise<Extension[]> {
  switch (language) {
    case 'json':
      return [json()]
    case 'yaml':
      return [yaml()]
    case 'markdown':
      return [markdown()]
    case 'bash':
      return [StreamLanguage.define(shell)]
    case 'toml':
      return [StreamLanguage.define(toml)]
    case 'dotenv':
      return [StreamLanguage.define(properties)]
    case 'javascript':
      return [javascript()]
    case 'typescript':
      return [javascript({ typescript: true })]
    default:
      return []
  }
}
