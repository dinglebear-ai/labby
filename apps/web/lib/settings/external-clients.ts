export const SUPPORTED_EXTERNAL_CLIENTS = [
  { value: 'codex', label: 'Codex' },
  { value: 'claude-code', label: 'Claude Code' },
] as const
export type ExternalClient = typeof SUPPORTED_EXTERNAL_CLIENTS[number]['value']
export type ClientConnection = 'saved-cli' | 'oauth'

function shellQuote(value: string): string {
  return "'" + value.replaceAll("'", "'\"'\"'") + "'"
}

export function externalClientCommand(clients: ExternalClient[], connection: ClientConnection, endpoint?: unknown): string | undefined {
  const selected = SUPPORTED_EXTERNAL_CLIENTS.filter(client => clients.includes(client.value)).map(client => client.value)
  if (!selected.length) return undefined
  const base = `labby setup clients connect --clients ${selected.join(',')}`
  if (connection === 'saved-cli') return base
  if (typeof endpoint !== 'string' || endpoint.trim() !== endpoint || Array.from(endpoint).some(character => character === '\\' || character.charCodeAt(0) <= 32 || character.charCodeAt(0) === 127)) return undefined
  try {
    const url = new URL(endpoint)
    if (!url.hostname || url.protocol !== 'https:' || url.pathname !== '/mcp'
      || url.username || url.password || url.search || url.hash) return undefined
    return `${base} --connection oauth --gateway-url ${shellQuote(url.toString())}`
  } catch { return undefined }
}
