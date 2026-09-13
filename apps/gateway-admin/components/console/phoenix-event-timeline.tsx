import { AlertTriangle, Brain, Check, CircleEllipsis, FileDiff, Gauge, ListChecks, Plug, Wrench } from 'lucide-react'
import type { PhoenixEvent } from '@/lib/api/phoenix-client'

type EventView = {
  kind: 'message' | 'reasoning' | 'tool' | 'plan' | 'diff' | 'usage' | 'warning' | 'error' | 'status'
  label: string
  detail?: string
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
}

function firstText(value: unknown, keys: string[]): string | undefined {
  const source = record(value)
  for (const key of keys) {
    const candidate = source[key]
    if (typeof candidate === 'string' && candidate.trim()) return candidate.trim()
  }
}

function nestedText(value: unknown): string | undefined {
  if (typeof value === 'string' && value.trim()) return value.trim()
  if (Array.isArray(value)) return value.map(nestedText).filter(Boolean).join('\n') || undefined
  const source = record(value)
  return firstText(source, ['text', 'delta', 'message', 'summary', 'title', 'content'])
    ?? (source.content === undefined ? undefined : nestedText(source.content))
    ?? (source.parts === undefined ? undefined : nestedText(source.parts))
}

function numberAt(value: unknown, keys: string[]): number | undefined {
  const source = record(value)
  for (const key of keys) if (typeof source[key] === 'number') return source[key]
}

function item(event: PhoenixEvent) {
  return record(record(event.params).item)
}

function summarize(event: PhoenixEvent): EventView | undefined {
  const params = record(event.params)
  const eventItem = item(event)
  const method = event.method.toLowerCase()
  const itemType = firstText(eventItem, ['type'])?.toLowerCase() ?? ''
  const text = nestedText(eventItem) ?? nestedText(params)
  if (method.includes('agentmessage') || method.includes('agent_message') || itemType.includes('agentmessage')) {
    return { kind: 'message', label: method.includes('delta') ? 'Response streaming' : 'Response updated', detail: text }
  }
  if (method.includes('reasoning') || itemType.includes('reasoning')) return { kind: 'reasoning', label: 'Reasoning', detail: text }
  if (method.includes('plan') || itemType.includes('plan')) return { kind: 'plan', label: 'Plan updated', detail: text }
  if (method.includes('diff') || itemType.includes('filechange') || itemType.includes('diff')) return { kind: 'diff', label: 'Changes prepared', detail: text }
  if (method.includes('mcp') || method.includes('tool') || itemType.includes('command') || itemType.includes('tool')) {
    const name = firstText(eventItem, ['name', 'tool', 'server']) ?? firstText(params, ['name', 'toolName', 'serverName'])
    const complete = method.includes('complete') || method.includes('end')
    return { kind: 'tool', label: name ? `${complete ? 'Used' : 'Using'} ${name}` : complete ? 'Tool completed' : 'Tool activity', detail: text }
  }
  if (method.includes('token') || method.includes('usage')) {
    const usage = record(params.tokenUsage ?? params.usage ?? params)
    const last = record(usage.last ?? usage.lastUsage ?? usage.last_usage)
    const total = record(usage.total ?? usage.totalUsage ?? usage.total_usage)
    const input = numberAt(usage, ['inputTokens', 'input_tokens']) ?? numberAt(last, ['inputTokens', 'input_tokens']) ?? numberAt(total, ['inputTokens', 'input_tokens'])
    const cached = numberAt(usage, ['cachedInputTokens', 'cached_input_tokens']) ?? numberAt(last, ['cachedInputTokens', 'cached_input_tokens']) ?? numberAt(total, ['cachedInputTokens', 'cached_input_tokens'])
    const output = numberAt(usage, ['outputTokens', 'output_tokens']) ?? numberAt(last, ['outputTokens', 'output_tokens']) ?? numberAt(total, ['outputTokens', 'output_tokens'])
    const detail = [typeof input === 'number' ? `${input.toLocaleString()} in` : '', typeof cached === 'number' ? `${cached.toLocaleString()} cached` : '', typeof output === 'number' ? `${output.toLocaleString()} out` : ''].filter(Boolean).join(' · ')
    return { kind: 'usage', label: 'Context usage', detail: detail || undefined }
  }
  if (method.includes('error') || itemType.includes('error')) return { kind: 'error', label: 'App Server error', detail: text }
  if (method.includes('warning')) return { kind: 'warning', label: 'App Server warning', detail: text }
  if (method === 'turn/completed') {
    const turn = record(params.turn)
    return { kind: turn.status === 'completed' ? 'status' : 'error', label: turn.status === 'completed' ? 'Turn completed' : 'Turn ended', detail: firstText(record(turn.error), ['message']) }
  }
  if (method.includes('status')) return { kind: 'status', label: 'Status updated', detail: text }
}

const styles = {
  message: ['text-aurora-accent-pink', CircleEllipsis],
  reasoning: ['text-aurora-accent-pink', Brain], tool: ['text-aurora-accent-primary', Wrench],
  plan: ['text-aurora-accent-strong', ListChecks], diff: ['text-aurora-status-success', FileDiff],
  usage: ['text-aurora-text-muted', Gauge], warning: ['text-aurora-warn', AlertTriangle],
  error: ['text-aurora-status-error', AlertTriangle], status: ['text-aurora-status-success', Check],
} as const

export function PhoenixEventTimeline({ events }: { events: PhoenixEvent[] }) {
  const visible = events.map(summarize).filter((event): event is EventView => event !== undefined)
  if (visible.length === 0) return null
  return <section aria-label="Phoenix activity" className="flex flex-col gap-1.5">
    <h3 className="flex items-center gap-1.5 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted"><CircleEllipsis size={12}/>Activity</h3>
    {visible.map((event, index) => {
      const [color, Icon] = styles[event.kind]
      return <div data-phoenix-event={event.kind} key={`${event.label}-${index}`} className="flex min-w-0 gap-2 rounded-[9px] border border-aurora-border-default/45 bg-[var(--gw0-0_40)] px-2.5 py-2">
        <Icon aria-hidden size={13} className={`mt-0.5 shrink-0 ${color}`}/><div className="min-w-0"><p className="text-[11px] font-bold text-aurora-text-primary">{event.label}</p>{event.detail && <p className="mt-0.5 line-clamp-3 whitespace-pre-wrap text-[10.5px] leading-relaxed text-aurora-text-muted">{event.detail}</p>}</div>
      </div>
    })}
  </section>
}

export function PhoenixRuntimeSummary({ mcpConfigured, protocol, runtimeVersion, capabilities, unsupported = [] }: { mcpConfigured?: boolean; protocol?: string; runtimeVersion?: string | null; capabilities?: string[]; unsupported?: string[] }) {
  const mcpLabel = mcpConfigured === true ? 'Labby MCP' : mcpConfigured === false ? 'MCP unavailable' : 'MCP status unknown'
  return <div aria-label="Phoenix runtime capabilities" className="flex min-w-0 items-center gap-1.5 overflow-hidden text-[9.5px] font-semibold text-aurora-text-muted">
    <span title={[runtimeVersion, protocol ? `protocol ${protocol}` : ''].filter(Boolean).join(' · ') || undefined} className="h-5 shrink-0 rounded-md border border-aurora-border-default/60 bg-[var(--gw0-0_45)] px-2 leading-5">{runtimeVersion ?? (protocol ? `v${protocol.split('-')[0]}` : 'App Server')}</span>
    <span title={mcpConfigured === true ? 'Labby MCP is configured over container loopback' : mcpConfigured === false ? 'Labby MCP loopback is not configured' : 'The server did not report MCP configuration'} className={`inline-flex h-5 shrink-0 items-center gap-1 rounded-md border px-2 leading-5 ${mcpConfigured === true ? 'border-aurora-status-success/25 bg-aurora-status-success/[.08] text-aurora-status-success' : mcpConfigured === false ? 'border-aurora-warn/25 bg-aurora-warn/[.08] text-aurora-warn' : 'border-aurora-border-default/60 bg-[var(--gw0-0_45)] text-aurora-text-muted'}`}><Plug size={10}/>{mcpLabel}</span>
    {capabilities && <span title={`Available: ${capabilities.join(', ')}${unsupported.length ? `. Unavailable: ${unsupported.join(', ')}` : ''}`} className="h-5 shrink-0 rounded-md border border-aurora-border-default/60 bg-[var(--gw0-0_45)] px-2 leading-5">{capabilities.length} capabilities</span>}
  </div>
}
