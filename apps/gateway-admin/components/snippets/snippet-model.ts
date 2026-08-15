/**
 * Pure helpers behind the Snippets screen body.
 *
 * Everything here works off what `/v1/snippets` actually returns — the list
 * shape (`SnippetInfo`) and the resolved body (`ResolvedSnippet.body`). The
 * Gateway Console mock decorates its snippet table with run counts, failure
 * counts, average runtime and per-run history sparklines; the API exposes none
 * of those, so the table renders `—` for them rather than inventing numbers.
 * The mock does the same thing wherever its own fixture lacks a value.
 */

import type { SnippetInfo, SnippetInputSpec } from '@/lib/types/snippets'

// ---------------------------------------------------------------------------
// Body parsing
// ---------------------------------------------------------------------------

const FRONTMATTER_RE = /^---\r?\n[\s\S]*?\r?\n---[ \t]*(?:\r?\n|$)/
const CODE_FENCE_RE = /(?:^|\r?\n)```(?:js|javascript|ts|typescript)\b[^\n]*\r?\n([\s\S]*?)\r?\n```/
const CALL_TOOL_RE = /callTool\(\s*(["'`])([^"'`\n]+)\1/g

export interface ParsedSnippetBody {
  /** The YAML frontmatter block including its `---` fences, when present. */
  frontmatter: string | null
  /** Markdown prose between the frontmatter and the first executable fence. */
  tutorial: string | null
  /**
   * What the mock's SOURCE block shows: frontmatter plus the executable arrow
   * function. Falls back to the whole body when there is no code fence.
   */
  source: string
  /** `upstream::tool` ids referenced by `callTool(...)` in the executable code. */
  tools: string[]
  /** Distinct upstream prefixes of {@link tools}. */
  servers: string[]
}

const EMPTY_BODY: ParsedSnippetBody = {
  frontmatter: null,
  tutorial: null,
  source: '',
  tools: [],
  servers: [],
}

export function parseSnippetBody(body: string | null | undefined): ParsedSnippetBody {
  if (!body || !body.trim()) return EMPTY_BODY

  const frontmatterMatch = FRONTMATTER_RE.exec(body)
  const frontmatter = frontmatterMatch ? frontmatterMatch[0].trimEnd() : null
  const rest = frontmatterMatch ? body.slice(frontmatterMatch[0].length) : body

  const codeMatch = CODE_FENCE_RE.exec(rest)
  const code = codeMatch?.[1]?.trim() ?? null

  const tutorial = rest.slice(0, codeMatch ? codeMatch.index : rest.length).trim() || null
  const source = code
    ? [frontmatter, code].filter((part): part is string => Boolean(part)).join('\n\n')
    : body.trim()

  const tools = extractToolIds(code ?? body)
  const servers = uniq(
    tools
      .filter((id) => id.includes('::'))
      .map((id) => id.slice(0, id.indexOf('::'))),
  )

  return { frontmatter, tutorial, source, tools, servers }
}

function extractToolIds(code: string): string[] {
  const ids: string[] = []
  CALL_TOOL_RE.lastIndex = 0
  let match = CALL_TOOL_RE.exec(code)
  while (match) {
    ids.push(match[2])
    match = CALL_TOOL_RE.exec(code)
  }
  return uniq(ids)
}

function uniq(values: string[]): string[] {
  return [...new Set(values)]
}

// ---------------------------------------------------------------------------
// Source highlighting
// ---------------------------------------------------------------------------

export type SnippetTokenKind = 'plain' | 'comment' | 'string' | 'keyword' | 'key' | 'meta'

export interface SnippetToken {
  kind: SnippetTokenKind
  text: string
}

/**
 * The mock colours `async` / `await` / `return` but leaves `const` and `let`
 * uncoloured, so this is control-flow and async only — not every reserved word.
 */
const KEYWORDS = new Set([
  'async',
  'await',
  'return',
  'function',
  'new',
  'throw',
  'yield',
  'typeof',
  'if',
  'else',
  'for',
  'while',
  'do',
  'switch',
  'case',
  'break',
  'continue',
  'try',
  'catch',
  'finally',
])

export function tokenizeSnippetSource(source: string): SnippetToken[] {
  if (!source) return []

  const tokens: SnippetToken[] = []
  const push = (kind: SnippetTokenKind, text: string) => {
    if (!text) return
    const last = tokens[tokens.length - 1]
    if (last && last.kind === kind) {
      last.text += text
      return
    }
    tokens.push({ kind, text })
  }

  let rest = source
  const frontmatterMatch = FRONTMATTER_RE.exec(source)
  if (frontmatterMatch) {
    tokenizeFrontmatter(frontmatterMatch[0], push)
    rest = source.slice(frontmatterMatch[0].length)
  }

  tokenizeCode(rest, push)
  return tokens
}

function tokenizeFrontmatter(
  block: string,
  push: (kind: SnippetTokenKind, text: string) => void,
): void {
  const trailingNewline = block.endsWith('\n')
  const lines = (trailingNewline ? block.slice(0, -1) : block).split('\n')
  lines.forEach((line, index) => {
    if (line.trimEnd() === '---') {
      push('meta', line)
    } else {
      const separator = line.indexOf(':')
      if (separator > 0) {
        push('key', line.slice(0, separator))
        push('plain', ':')
        push('string', line.slice(separator + 1))
      } else {
        push('plain', line)
      }
    }
    if (index < lines.length - 1 || trailingNewline) push('plain', '\n')
  })
}

function tokenizeCode(code: string, push: (kind: SnippetTokenKind, text: string) => void): void {
  let index = 0
  while (index < code.length) {
    const char = code[index]

    if (char === '/' && code[index + 1] === '/') {
      const end = code.indexOf('\n', index)
      const stop = end === -1 ? code.length : end
      push('comment', code.slice(index, stop))
      index = stop
      continue
    }

    if (char === '/' && code[index + 1] === '*') {
      const end = code.indexOf('*/', index + 2)
      const stop = end === -1 ? code.length : end + 2
      push('comment', code.slice(index, stop))
      index = stop
      continue
    }

    if (char === '"' || char === "'" || char === '`') {
      let cursor = index + 1
      while (cursor < code.length) {
        if (code[cursor] === '\\') {
          cursor += 2
          continue
        }
        if (code[cursor] === char) {
          cursor += 1
          break
        }
        cursor += 1
      }
      push('string', code.slice(index, Math.min(cursor, code.length)))
      index = Math.min(cursor, code.length)
      continue
    }

    if (/[A-Za-z_$]/.test(char)) {
      let cursor = index
      while (cursor < code.length && /[A-Za-z0-9_$]/.test(code[cursor])) cursor += 1
      const word = code.slice(index, cursor)
      push(KEYWORDS.has(word) ? 'keyword' : 'plain', word)
      index = cursor
      continue
    }

    push('plain', char)
    index += 1
  }
}

// ---------------------------------------------------------------------------
// Filtering / sorting
// ---------------------------------------------------------------------------

export type SnippetSortDirection = 'asc' | 'desc'

export function snippetKey(snippet: Pick<SnippetInfo, 'source' | 'name'>): string {
  return `${snippet.source}:${snippet.name}`
}

/** Tag pills, most-used first — the mock's filter row is ordered by weight. */
export function collectSnippetTags(snippets: SnippetInfo[]): string[] {
  const counts = new Map<string, number>()
  for (const snippet of snippets) {
    for (const tag of snippet.tags ?? []) {
      counts.set(tag, (counts.get(tag) ?? 0) + 1)
    }
  }
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([tag]) => tag)
}

export function filterSnippets(
  snippets: SnippetInfo[],
  query: string,
  tag: string | null,
): SnippetInfo[] {
  const needle = query.trim().toLowerCase()
  return snippets.filter((snippet) => {
    if (tag && !(snippet.tags ?? []).includes(tag)) return false
    if (!needle) return true
    const haystack = [snippet.name, snippet.description ?? '', ...(snippet.tags ?? [])]
      .join(' ')
      .toLowerCase()
    return haystack.includes(needle)
  })
}

export function sortSnippetsByName(
  snippets: SnippetInfo[],
  direction: SnippetSortDirection,
): SnippetInfo[] {
  const sign = direction === 'asc' ? 1 : -1
  return [...snippets].sort((a, b) => sign * a.name.localeCompare(b.name))
}

// ---------------------------------------------------------------------------
// Typed input values
// ---------------------------------------------------------------------------

export type SnippetParamsResult =
  | { ok: true; params: Record<string, unknown> }
  | { ok: false; error: string }

/** Placeholder text for an input field — the declared default, like the mock. */
export function inputPlaceholder(spec: SnippetInputSpec): string {
  if (spec.default === undefined || spec.default === null) return ''
  if (typeof spec.default === 'string') return spec.default
  return JSON.stringify(spec.default)
}

/**
 * Coerce the raw string values typed into the INPUTS table into the types the
 * snippet declares. Blank fields are omitted so the runtime default applies.
 */
export function buildSnippetParams(
  inputs: Record<string, SnippetInputSpec> | undefined,
  values: Record<string, string> | undefined,
): SnippetParamsResult {
  const params: Record<string, unknown> = {}
  for (const [name, spec] of Object.entries(inputs ?? {})) {
    const raw = (values?.[name] ?? '').trim()
    if (!raw) continue

    switch (spec.ty) {
      case 'integer': {
        const parsed = Number(raw)
        if (!Number.isInteger(parsed)) return { ok: false, error: `Input \`${name}\` must be an integer.` }
        params[name] = parsed
        break
      }
      case 'number': {
        const parsed = Number(raw)
        if (!Number.isFinite(parsed)) return { ok: false, error: `Input \`${name}\` must be a number.` }
        params[name] = parsed
        break
      }
      case 'boolean': {
        const lowered = raw.toLowerCase()
        if (['true', '1', 'yes'].includes(lowered)) params[name] = true
        else if (['false', '0', 'no'].includes(lowered)) params[name] = false
        else return { ok: false, error: `Input \`${name}\` must be true or false.` }
        break
      }
      case 'object':
      case 'array':
      case 'json': {
        let parsed: unknown
        try {
          parsed = JSON.parse(raw)
        } catch {
          return { ok: false, error: `Input \`${name}\` must be valid JSON.` }
        }
        if (spec.ty === 'array' && !Array.isArray(parsed)) {
          return { ok: false, error: `Input \`${name}\` must be a JSON array.` }
        }
        if (spec.ty === 'object' && (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed))) {
          return { ok: false, error: `Input \`${name}\` must be a JSON object.` }
        }
        params[name] = parsed
        break
      }
      default:
        params[name] = raw
    }
  }
  return { ok: true, params }
}
