const INITIALISMS = new Set([
  'api',
  'cli',
  'cpu',
  'gpu',
  'http',
  'https',
  'id',
  'json',
  'mcp',
  'os',
  'pgid',
  'pid',
  'qa',
  'sdk',
  'sql',
  'ssh',
  'ui',
  'uri',
  'url',
  'vm',
  'xml',
])

const LOWERCASE_WORDS = new Set(['and', 'for', 'from', 'in', 'of', 'on', 'the', 'to', 'with'])

/** Presentation only; the configured identifier remains the server key everywhere else. */
export function gatewayDisplayName(identifier: string): string {
  if (!/[_-]/.test(identifier)) return identifier

  return identifier
    .split(/[_-]+/)
    .filter(Boolean)
    .map((word, index) => {
      const normalized = word.toLowerCase()
      if (INITIALISMS.has(normalized)) return normalized.toUpperCase()
      if (index > 0 && LOWERCASE_WORDS.has(normalized)) return normalized
      return word[0]!.toUpperCase() + word.slice(1)
    })
    .join(' ')
}
