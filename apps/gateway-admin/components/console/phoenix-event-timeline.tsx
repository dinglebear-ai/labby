'use client'

import { useMemo, useState } from 'react'
import { AlertTriangle, Bot, Brain, Check, ChevronDown, FilePenLine, Gauge, Globe2, ListChecks, PlugZap, Sparkles, Terminal, Workflow, Wrench } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { PhoenixEvent } from '@/lib/api/phoenix-client'
import { cn } from '@/lib/utils'

type EventKind = 'reasoning' | 'mcp' | 'command' | 'file' | 'subagent' | 'hook' | 'web' | 'skill' | 'plan' | 'usage' | 'warning' | 'error' | 'status' | 'other'

type EventView = {
  kind: EventKind
  key: string
  label: string
  detail?: string
  status?: string
}

type EventCluster = EventView & { entries: EventView[] }
type TimelineEntry = { kind: 'single'; cluster: EventCluster } | { kind: 'tools'; clusters: EventCluster[] }

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

function nestedText(value: unknown, depth = 0): string | undefined {
  if (depth > 4) return undefined
  if (typeof value === 'string' && value.trim()) return value.trim()
  if (Array.isArray(value)) return value.map((entry) => nestedText(entry, depth + 1)).filter(Boolean).join('\n') || undefined
  const source = record(value)
  return firstText(source, ['text', 'delta', 'message', 'summary', 'title', 'content', 'query', 'command'])
    ?? (source.content === undefined ? undefined : nestedText(source.content, depth + 1))
    ?? (source.parts === undefined ? undefined : nestedText(source.parts, depth + 1))
}

function numberAt(value: unknown, keys: string[]): number | undefined {
  const source = record(value)
  for (const key of keys) if (typeof source[key] === 'number') return source[key]
}

function item(event: PhoenixEvent) {
  return record(record(event.params).item)
}

export function phoenixEventTime(event: PhoenixEvent): number | undefined {
  return typeof event.received_at_ms === 'number' ? event.received_at_ms : undefined
}

export function phoenixEventTurnId(event: PhoenixEvent): string | undefined {
  const params = record(event.params)
  return firstText(params, ['turnId', 'turn_id']) ?? firstText(record(params.turn), ['id'])
}

export function isPhoenixAgentDelta(event: PhoenixEvent): boolean {
  return event.method === 'item/agentMessage/delta'
}

export function phoenixAgentDelta(event: PhoenixEvent): string {
  const delta = record(event.params).delta
  return typeof delta === 'string' ? delta : ''
}

function containsType(value: unknown, wanted: string, depth = 0): boolean {
  if (depth > 5) return false
  if (Array.isArray(value)) return value.some((entry) => containsType(entry, wanted, depth + 1))
  const source = record(value)
  if (source.type === wanted) return true
  return Object.values(source).some((entry) => entry !== null && typeof entry === 'object' && containsType(entry, wanted, depth + 1))
}

function latestTokenUsage(events: PhoenixEvent[]): Record<string, unknown> | undefined {
  const latest = [...events].reverse().find((event) => event.method === 'thread/tokenUsage/updated' || event.method.toLowerCase().includes('usage'))
  if (!latest) return undefined
  const params = record(latest.params)
  return record(params.tokenUsage ?? params.usage ?? params)
}

export function phoenixTotalTokens(events: PhoenixEvent[]): number | undefined {
  const usage = latestTokenUsage(events)
  if (!usage) return undefined
  const total = record(usage.total ?? usage.totalUsage ?? usage.total_usage)
  const direct = numberAt(total, ['totalTokens', 'total_tokens']) ?? numberAt(usage, ['totalTokens', 'total_tokens'])
  if (direct !== undefined) return direct
  const last = record(usage.last ?? usage.lastUsage ?? usage.last_usage)
  const input = numberAt(last, ['inputTokens', 'input_tokens']) ?? numberAt(usage, ['inputTokens', 'input_tokens']) ?? 0
  const output = numberAt(last, ['outputTokens', 'output_tokens']) ?? numberAt(usage, ['outputTokens', 'output_tokens']) ?? 0
  return input + output || undefined
}

export function phoenixContextWindow(events: PhoenixEvent[]): number | undefined {
  const usage = latestTokenUsage(events)
  if (!usage) return undefined
  return numberAt(usage, ['modelContextWindow', 'model_context_window', 'contextWindow', 'context_window'])
}

function summarize(event: PhoenixEvent, index: number): EventView | undefined {
  const params = record(event.params)
  const eventItem = item(event)
  const method = event.method.toLowerCase()
  const itemType = firstText(eventItem, ['type'])?.toLowerCase() ?? ''
  const text = nestedText(eventItem) ?? nestedText(params)
  const id = firstText(eventItem, ['id', 'callId', 'call_id']) ?? `${method}:${index}`

  if (method.includes('agentmessage') || method.includes('agent_message') || itemType.includes('agentmessage')) return undefined
  if (method.includes('reasoning') || itemType.includes('reasoning')) return { kind: 'reasoning', key: 'reasoning:' + id, label: 'Reasoning', detail: text }
  if (method === 'hook/started' || method === 'hook/completed') {
    const run = record(params.run)
    const name = firstText(run, ['name', 'event', 'hookName', 'hook_name']) ?? 'Hook'
    return { kind: 'hook', key: 'hook:' + name, label: name, detail: nestedText(run) ?? text, status: method.endsWith('completed') ? 'completed' : 'running' }
  }
  if (containsType(eventItem, 'skill')) {
    const name = firstText(eventItem, ['name', 'skill']) ?? firstText(params, ['name', 'skill']) ?? 'Skill'
    return { kind: 'skill', key: 'skill:' + name, label: name, detail: text }
  }
  if (itemType === 'collabtoolcall' || method.includes('collab')) {
    const name = firstText(eventItem, ['receiverThreadId', 'agentName', 'tool', 'name']) ?? 'Subagent'
    return { kind: 'subagent', key: 'subagent:' + id, label: name, detail: text, status: firstText(eventItem, ['status']) }
  }
  if (itemType === 'mcptoolcall' || method.includes('mcptoolcall')) {
    const server = firstText(eventItem, ['server', 'serverName']) ?? firstText(params, ['server', 'serverName'])
    const tool = firstText(eventItem, ['tool', 'toolName', 'name']) ?? firstText(params, ['tool', 'toolName', 'name'])
    const name = [server, tool].filter(Boolean).join(' · ') || 'MCP tool'
    return { kind: 'mcp', key: 'mcp:' + id, label: name, detail: text, status: firstText(eventItem, ['status']) }
  }
  if (itemType === 'dynamictoolcall' || method.includes('dynamictool')) {
    const name = firstText(eventItem, ['tool', 'name']) ?? 'Tool'
    return { kind: 'other', key: 'tool:' + id, label: name, detail: text, status: firstText(eventItem, ['status']) }
  }
  if (itemType === 'commandexecution' || method.includes('commandexecution')) return { kind: 'command', key: 'command:' + id, label: 'Command', detail: text, status: firstText(eventItem, ['status']) }
  if (itemType === 'filechange' || method.includes('diff')) return { kind: 'file', key: 'file:change', label: 'File change', detail: text, status: firstText(eventItem, ['status']) }
  if (itemType === 'websearch' || method.includes('websearch')) return { kind: 'web', key: 'web:search', label: 'Web search', detail: text, status: firstText(eventItem, ['status']) }
  if (method.includes('plan') || itemType.includes('plan')) return { kind: 'plan', key: 'plan', label: 'Plan', detail: text }
  if (method.includes('token') || method.includes('usage')) {
    const usage = record(params.tokenUsage ?? params.usage ?? params)
    const last = record(usage.last ?? usage.lastUsage ?? usage.last_usage)
    const total = record(usage.total ?? usage.totalUsage ?? usage.total_usage)
    const totalTokens = numberAt(total, ['totalTokens', 'total_tokens']) ?? numberAt(usage, ['totalTokens', 'total_tokens'])
    const input = numberAt(usage, ['inputTokens', 'input_tokens']) ?? numberAt(last, ['inputTokens', 'input_tokens']) ?? numberAt(total, ['inputTokens', 'input_tokens'])
    const cached = numberAt(usage, ['cachedInputTokens', 'cached_input_tokens']) ?? numberAt(last, ['cachedInputTokens', 'cached_input_tokens']) ?? numberAt(total, ['cachedInputTokens', 'cached_input_tokens'])
    const output = numberAt(usage, ['outputTokens', 'output_tokens']) ?? numberAt(last, ['outputTokens', 'output_tokens']) ?? numberAt(total, ['outputTokens', 'output_tokens'])
    const detail = [typeof totalTokens === 'number' ? totalTokens.toLocaleString() + ' total' : '', typeof input === 'number' ? input.toLocaleString() + ' in' : '', typeof cached === 'number' ? cached.toLocaleString() + ' cached' : '', typeof output === 'number' ? output.toLocaleString() + ' out' : ''].filter(Boolean).join(' · ')
    return { kind: 'usage', key: 'usage', label: 'Context usage', detail: detail || undefined }
  }
  if (method.includes('error') || itemType.includes('error')) return { kind: 'error', key: 'error', label: 'Error', detail: text }
  if (method.includes('warning') || method.includes('safety')) return { kind: 'warning', key: 'warning', label: method.includes('safety') ? 'Safety check' : 'Warning', detail: text }
  if (method.includes('model/rerouted')) return { kind: 'status', key: 'model:rerouted', label: 'Model rerouted', detail: text }
  if (method.includes('model/verification')) return { kind: 'status', key: 'model:verification', label: 'Model verification', detail: text }
  if (itemType === 'contextcompaction') return { kind: 'status', key: 'context:compaction', label: 'Context compacted', detail: text }
  if (method === 'turn/completed') {
    const turn = record(params.turn)
    return { kind: turn.status === 'completed' ? 'status' : 'error', key: 'turn:completed', label: turn.status === 'completed' ? 'Turn completed' : 'Turn ended', detail: firstText(record(turn.error), ['message']) }
  }
  if (method.includes('status')) return { kind: 'status', key: 'status:' + method, label: 'Status', detail: text }
  if (method === 'item/started' || method === 'item/completed') return { kind: 'other', key: 'item:' + (itemType || id), label: itemType || 'Activity', detail: text, status: firstText(eventItem, ['status']) }
}

const STYLE: Record<EventKind, { icon: LucideIcon; tone: string; ring: string; glow: string }> = {
  reasoning: { icon: Brain, tone: 'text-aurora-accent-pink', ring: 'border-aurora-accent-pink/45', glow: 'bg-aurora-accent-pink/10' },
  mcp: { icon: PlugZap, tone: 'text-aurora-accent-primary', ring: 'border-aurora-accent-primary/45', glow: 'bg-aurora-accent-primary/10' },
  command: { icon: Terminal, tone: 'text-aurora-warn', ring: 'border-aurora-warn/45', glow: 'bg-aurora-warn/10' },
  file: { icon: FilePenLine, tone: 'text-aurora-status-success', ring: 'border-aurora-status-success/45', glow: 'bg-aurora-status-success/10' },
  subagent: { icon: Bot, tone: 'text-[#c99cff]', ring: 'border-[#c99cff]/45', glow: 'bg-[#c99cff]/10' },
  hook: { icon: Workflow, tone: 'text-[#72dcc5]', ring: 'border-[#72dcc5]/45', glow: 'bg-[#72dcc5]/10' },
  web: { icon: Globe2, tone: 'text-[#63bff8]', ring: 'border-[#63bff8]/45', glow: 'bg-[#63bff8]/10' },
  skill: { icon: Sparkles, tone: 'text-[#e8c66b]', ring: 'border-[#e8c66b]/45', glow: 'bg-[#e8c66b]/10' },
  plan: { icon: ListChecks, tone: 'text-aurora-accent-strong', ring: 'border-aurora-accent-primary/35', glow: 'bg-aurora-accent-primary/[.07]' },
  usage: { icon: Gauge, tone: 'text-aurora-text-muted', ring: 'border-aurora-border-strong', glow: 'bg-[var(--gw0-0_40)]' },
  warning: { icon: AlertTriangle, tone: 'text-aurora-warn', ring: 'border-aurora-warn/45', glow: 'bg-aurora-warn/10' },
  error: { icon: AlertTriangle, tone: 'text-aurora-status-error', ring: 'border-aurora-status-error/45', glow: 'bg-aurora-status-error/10' },
  status: { icon: Check, tone: 'text-aurora-status-success', ring: 'border-aurora-status-success/35', glow: 'bg-aurora-status-success/[.07]' },
  other: { icon: Wrench, tone: 'text-aurora-text-muted', ring: 'border-aurora-border-strong', glow: 'bg-[var(--gw0-0_40)]' },
}

function clusterEvents(events: PhoenixEvent[]): EventCluster[] {
  const visible = events.map(summarize).filter((entry): entry is EventView => entry !== undefined)
  const result: EventCluster[] = []
  for (const entry of visible) {
    const previous = result.at(-1)
    if (previous && previous.key === entry.key && previous.kind === entry.kind) {
      previous.entries.push(entry)
      previous.detail = entry.detail ?? previous.detail
      previous.status = entry.status ?? previous.status
    } else {
      result.push({ ...entry, entries: [entry] })
    }
  }
  return result
}

const TOOL_KINDS = new Set<EventKind>(['mcp', 'command', 'web', 'subagent'])

function groupConsecutiveTools(clusters: EventCluster[]): TimelineEntry[] {
  const result: TimelineEntry[] = []
  for (const cluster of clusters) {
    const previous = result.at(-1)
    if (TOOL_KINDS.has(cluster.kind)) {
      if (previous?.kind === 'tools') previous.clusters.push(cluster)
      else result.push({ kind: 'tools', clusters: [cluster] })
    } else {
      result.push({ kind: 'single', cluster })
    }
  }
  return result.flatMap((entry) => entry.kind === 'tools' && entry.clusters.length === 1
    ? [{ kind: 'single' as const, cluster: entry.clusters[0] }]
    : [entry])
}

function ToolGroupNode({ clusters }: { clusters: EventCluster[] }) {
  const [expanded, setExpanded] = useState(false)
  const count = clusters.length
  const tools = clusters.reduce<{ kind: EventKind; label: string; count: number }[]>((items, cluster) => {
    const existing = items.find((item) => item.kind === cluster.kind && item.label === cluster.label)
    if (existing) existing.count += 1
    else items.push({ kind: cluster.kind, label: cluster.label, count: 1 })
    return items
  }, [])
  const description = tools.map((tool) => `${tool.label}${tool.count > 1 ? `, ${tool.count} calls` : ''}`).join('; ')
  return <div data-phoenix-tool-group={count} className="min-w-0 max-w-full rounded-[11px] border border-aurora-border-default bg-aurora-control-surface">
    <button type="button" aria-expanded={expanded} aria-label={`${count} tool calls: ${description}`} title={description} onClick={() => setExpanded((value) => !value)} className="flex max-w-full flex-wrap items-center gap-2 rounded-[11px] px-2 py-2 text-left hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">
      {tools.map((tool) => {
        const token = STYLE[tool.kind]
        const Icon = token.icon
        return <span key={`${tool.kind}:${tool.label}`} data-phoenix-tool={tool.label} title={tool.label} className={cn('relative grid size-[31px] shrink-0 place-items-center rounded-[9px] border', token.tone, token.ring, token.glow)}>
          <Icon size={14} strokeWidth={1.8}/>
          {tool.count > 1 && <span data-phoenix-tool-count={tool.count} aria-hidden="true" className="absolute -right-1.5 -top-1.5 grid min-w-4 h-4 place-items-center rounded-full border border-aurora-border-strong bg-aurora-panel-strong px-0.5 text-[9px] font-bold leading-none tabular-nums text-aurora-text-primary">{tool.count}</span>}
        </span>
      })}
      <ChevronDown size={13} aria-hidden="true" className={cn('ml-0.5 shrink-0 text-aurora-text-muted transition-transform', expanded && 'rotate-180')}/>
    </button>
    {expanded && <ol className="border-t border-aurora-border-default px-3 py-1.5">
      {clusters.map((cluster, index) => <li key={cluster.key + '-' + index} className="min-w-0 border-b border-aurora-border-default py-2 last:border-b-0">
        <div className="flex min-w-0 items-center gap-2"><span className="min-w-0 flex-1 truncate text-[11.5px] font-semibold text-aurora-text-primary">{cluster.label}</span>{cluster.status && <span className="shrink-0 text-[10px] text-aurora-text-muted">{cluster.status}</span>}</div>
        {cluster.detail && <pre className="aurora-scrollbar mt-1 max-h-36 overflow-auto whitespace-pre-wrap break-words font-mono text-[10.5px] leading-[1.5] text-aurora-text-muted">{cluster.detail}</pre>}
      </li>)}
    </ol>}
  </div>
}

function EventNode({ cluster, last }: { cluster: EventCluster; last: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const token = STYLE[cluster.kind]
  const Icon = token.icon
  const grouped = cluster.entries.length > 1
  const countLabel = String(cluster.entries.length) + (cluster.entries.length === 1 ? ' event' : ' events')
  return <div data-phoenix-event={cluster.kind} className="relative flex min-h-8 items-start">
    {!last && <span aria-hidden className="absolute left-[15px] top-[29px] h-[calc(100%-17px)] w-px bg-gradient-to-b from-aurora-border-strong via-aurora-border-default to-transparent"/>}
    <button type="button" aria-expanded={expanded} aria-label={cluster.label + '. ' + countLabel} title={cluster.label} onClick={() => setExpanded((value) => !value)} className={cn('group/node relative z-10 grid size-[31px] shrink-0 place-items-center rounded-[9px] border transition-[transform,background-color,border-color,box-shadow] duration-150 hover:-translate-y-px hover:shadow-[0_5px_14px_-7px_currentColor] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary', token.tone, token.ring, token.glow)}>
      <Icon size={14} strokeWidth={1.8}/>{grouped && <span aria-hidden className="absolute -right-[3px] -top-[3px] grid size-[8px] place-items-center rounded-full border border-current bg-aurora-panel-strong"><span className="size-[2px] rounded-full bg-current"/></span>}
    </button>
    {expanded && <div className="ml-2.5 min-w-0 max-w-[min(520px,calc(100vw-110px))] rounded-[10px] border border-aurora-border-default bg-[var(--gw0-0_46)] px-3 py-2 shadow-[0_8px_28px_-22px_rgba(0,0,0,.8)]">
      <div className="flex min-w-0 items-center gap-2"><strong className={cn('truncate text-[11.5px]', token.tone)}>{cluster.label}</strong>{cluster.status && <span className="rounded border border-aurora-border-default px-1.5 py-px text-[8px] font-bold uppercase tracking-[.08em] text-aurora-text-muted">{cluster.status}</span>}<span className="flex-1"/>{grouped && <span className="text-[9px] tabular-nums text-aurora-text-muted">×{cluster.entries.length}</span>}<ChevronDown size={12} className="rotate-180 text-aurora-text-muted"/></div>
      {cluster.detail && <pre className="aurora-scrollbar mt-1.5 max-h-36 overflow-auto whitespace-pre-wrap break-words font-mono text-[10.5px] leading-[1.55] text-aurora-text-muted">{cluster.detail}</pre>}
    </div>}
  </div>
}

/** Icon-only, graph-like activity rail. Details are collapsed until a node is opened. */
export function PhoenixEventTimeline({ events, className }: { events: PhoenixEvent[]; className?: string }) {
  const grouped = useMemo(() => groupConsecutiveTools(clusterEvents(events)), [events])
  if (grouped.length === 0) return null
  return <section aria-label="Phoenix activity" className={cn('inline-flex w-fit max-w-full flex-col gap-0.5 pl-[2px]', className)}>
    {grouped.map((entry, index) => entry.kind === 'tools'
      ? <ToolGroupNode key={'tools-' + index} clusters={entry.clusters}/>
      : <EventNode key={entry.cluster.key + '-' + index} cluster={entry.cluster} last={index === grouped.length - 1}/>) }
  </section>
}

export function PhoenixRuntimeSummary({ mcpConfigured, protocol, runtimeVersion, capabilities, unsupported = [] }: { mcpConfigured?: boolean; protocol?: string; runtimeVersion?: string | null; capabilities?: string[]; unsupported?: string[] }) {
  const mcpLabel = mcpConfigured === true ? 'Labby MCP' : mcpConfigured === false ? 'MCP unavailable' : 'MCP status unknown'
  const runtimeTitle = [runtimeVersion, protocol ? 'protocol ' + protocol : ''].filter(Boolean).join(' · ') || undefined
  const capabilityTitle = capabilities ? 'Available: ' + capabilities.join(', ') + (unsupported.length ? '. Unavailable: ' + unsupported.join(', ') : '') : undefined
  return <div aria-label="Phoenix runtime capabilities" className="flex min-w-0 items-center gap-1.5 overflow-hidden text-[9.5px] font-semibold text-aurora-text-muted">
    <span title={runtimeTitle} className="h-5 shrink-0 rounded-md border border-aurora-border-default/60 bg-[var(--gw0-0_45)] px-2 leading-5">{runtimeVersion ?? (protocol ? 'v' + protocol.split('-')[0] : 'App Server')}</span>
    <span title={mcpConfigured === true ? 'Labby MCP is configured over container loopback' : mcpConfigured === false ? 'Labby MCP loopback is not configured' : 'The server did not report MCP configuration'} className={cn('inline-flex h-5 shrink-0 items-center gap-1 rounded-md border px-2 leading-5', mcpConfigured === true ? 'border-aurora-status-success/25 bg-aurora-status-success/[.08] text-aurora-status-success' : mcpConfigured === false ? 'border-aurora-warn/25 bg-aurora-warn/[.08] text-aurora-warn' : 'border-aurora-border-default/60 bg-[var(--gw0-0_45)] text-aurora-text-muted')}><PlugZap size={10}/>{mcpLabel}</span>
    {capabilities && <span title={capabilityTitle} className="h-5 shrink-0 rounded-md border border-aurora-border-default/60 bg-[var(--gw0-0_45)] px-2 leading-5">{capabilities.length} capabilities</span>}
  </div>
}
