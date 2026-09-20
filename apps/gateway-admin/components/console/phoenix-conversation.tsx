'use client'

import type { ReactNode } from 'react'
import { Clipboard, Pencil, RefreshCw } from 'lucide-react'
import type { PhoenixEvent, PhoenixMessage } from '@/lib/api/phoenix-client'
import { PhoenixEventTimeline, isPhoenixAgentDelta, phoenixAgentDelta, phoenixEventTime } from './phoenix-event-timeline'

type MessageEntry = { kind: 'message'; time: number; order: number; index: number; message: PhoenixMessage }
type EventEntry = { kind: 'event'; time: number; order: number; event: PhoenixEvent }
type Entry = MessageEntry | EventEntry

type Chunk =
  | { kind: 'message'; id: string; index: number; message: PhoenixMessage }
  | { kind: 'assistant-stream'; id: string; text: string; turnKey: string; messageIndex?: number }
  | { kind: 'events'; id: string; events: PhoenixEvent[] }

function eventTurnKey(event: PhoenixEvent): string {
  const params = event.params && typeof event.params === 'object' ? event.params as Record<string, unknown> : {}
  const turn = params.turn && typeof params.turn === 'object' ? params.turn as Record<string, unknown> : {}
  return String(params.turnId ?? params.turn_id ?? turn.id ?? 'active')
}

function buildChunks(messages: PhoenixMessage[], events: PhoenixEvent[]): Chunk[] {
  const timedEvents = events.filter((event) => phoenixEventTime(event) !== undefined)
  const legacyEvents = events.filter((event) => phoenixEventTime(event) === undefined)
  const suppressAssistant = new Set<number>()
  let priorAssistantTime = -Infinity
  for (let index = 0; index < messages.length; index += 1) {
    const message = messages[index]
    if (message.role !== 'assistant' || typeof message.created_at_ms !== 'number') continue
    if (timedEvents.some((event) => isPhoenixAgentDelta(event) && (phoenixEventTime(event) ?? 0) > priorAssistantTime && (phoenixEventTime(event) ?? Infinity) <= message.created_at_ms!)) suppressAssistant.add(index)
    priorAssistantTime = message.created_at_ms
  }

  const entries: Entry[] = []
  messages.forEach((message, index) => {
    if (suppressAssistant.has(index)) return
    entries.push({ kind: 'message', time: message.created_at_ms ?? Number.MAX_SAFE_INTEGER - messages.length + index, order: index * 10, index, message })
  })
  timedEvents.forEach((event, index) => entries.push({ kind: 'event', time: phoenixEventTime(event) ?? 0, order: typeof event.sequence === 'number' ? event.sequence : index, event }))
  entries.sort((left, right) => left.time - right.time || left.order - right.order)

  const chunks: Chunk[] = []
  const flushEvent = (event: PhoenixEvent) => {
    const previous = chunks.at(-1)
    if (previous?.kind === 'events') previous.events.push(event)
    else chunks.push({ kind: 'events', id: 'events-' + String(chunks.length), events: [event] })
  }
  for (const entry of entries) {
    if (entry.kind === 'message') {
      chunks.push({ kind: 'message', id: 'message-' + String(entry.index), index: entry.index, message: entry.message })
      continue
    }
    if (isPhoenixAgentDelta(entry.event)) {
      const delta = phoenixAgentDelta(entry.event)
      if (!delta) continue
      const turnKey = eventTurnKey(entry.event)
      const previous = chunks.at(-1)
      if (previous?.kind === 'assistant-stream' && previous.turnKey === turnKey) previous.text += delta
      else chunks.push({ kind: 'assistant-stream', id: 'stream-' + String(chunks.length), text: delta, turnKey })
      continue
    }
    flushEvent(entry.event)
  }
  if (legacyEvents.length) chunks.push({ kind: 'events', id: 'legacy-events', events: legacyEvents })

  for (const index of suppressAssistant) {
    const message = messages[index]
    const time = message.created_at_ms ?? Infinity
    for (let chunkIndex = chunks.length - 1; chunkIndex >= 0; chunkIndex -= 1) {
      const chunk = chunks[chunkIndex]
      if (chunk.kind === 'assistant-stream') {
        chunk.messageIndex = index
        break
      }
      if (chunk.kind === 'message' && typeof chunk.message.created_at_ms === 'number' && chunk.message.created_at_ms < time && chunk.message.role === 'user') break
    }
  }
  return chunks
}

function Reactions({ message, index, copiedIndex, onRetry, onCopy, onEdit }: { message: PhoenixMessage; index: number; copiedIndex?: number; onRetry: (index: number) => void; onCopy: (text: string, index: number) => void; onEdit?: (index: number, text: string) => void }) {
  return <div data-phoenix-reactions className="flex min-h-8 items-center gap-1 opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-within:opacity-100">
    {message.created_at_ms ? <time dateTime={new Date(message.created_at_ms).toISOString()} className="mr-1 text-[10.5px] tabular-nums text-aurora-text-muted">{new Date(message.created_at_ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}</time> : null}
    {onEdit && <button type="button" onClick={() => onEdit(index, message.text)} aria-label="Edit message" title="Edit in a new thread" className="grid size-7 place-items-center rounded-lg text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-text-primary"><Pencil size={13}/></button>}
    <button type="button" onClick={() => onRetry(index)} aria-label={message.role === 'assistant' ? 'Regenerate' : 'Retry from here'} title={message.role === 'assistant' ? 'Regenerate' : 'Retry from here'} className="grid size-7 place-items-center rounded-lg text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-accent-pink"><RefreshCw size={13}/></button>
    <button type="button" onClick={() => onCopy(message.text, index)} aria-label={message.role === 'assistant' ? 'Copy answer' : 'Copy message'} title="Copy" className="grid size-7 place-items-center rounded-lg text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-text-primary">{copiedIndex === index ? <span aria-hidden>✓</span> : <Clipboard size={13}/>}</button>
  </div>
}

export function PhoenixConversation({ messages, events, mark, copiedIndex, onRetry, onCopy, onEdit }: { messages: PhoenixMessage[]; events: PhoenixEvent[]; mark: ReactNode; copiedIndex?: number; onRetry: (index: number) => void; onCopy: (text: string, index: number) => void; onEdit: (index: number, text: string) => void }) {
  const chunks = buildChunks(messages, events)
  const streamedTurns = new Set<string>()
  return <>
    {chunks.map((chunk) => {
      if (chunk.kind === 'events') return <div key={chunk.id} className="pl-[35px]"><PhoenixEventTimeline events={chunk.events}/></div>
      if (chunk.kind === 'assistant-stream') {
        const firstForTurn = !streamedTurns.has(chunk.turnKey)
        streamedTurns.add(chunk.turnKey)
        const finalMessage = chunk.messageIndex === undefined ? undefined : messages[chunk.messageIndex]
        return <div data-phoenix-message="assistant-stream" key={chunk.id} className="group flex min-w-0 shrink-0 gap-[9px]">
          {firstForTurn ? <span className="grid size-[26px] shrink-0 place-items-center rounded-[9px] border border-aurora-accent-pink/40 bg-aurora-accent-pink/10 text-aurora-accent-pink">{mark}</span> : <span aria-hidden className="w-[26px] shrink-0"/>}
          <div className="min-w-0 flex-1"><div className="whitespace-pre-wrap text-[12.5px] font-semibold leading-[1.68] text-aurora-text-primary">{chunk.text}</div>{finalMessage && chunk.messageIndex !== undefined ? <Reactions message={finalMessage} index={chunk.messageIndex} copiedIndex={copiedIndex} onRetry={onRetry} onCopy={onCopy}/> : null}</div>
        </div>
      }
      const message = chunk.message
      if (message.role === 'user') return <div data-phoenix-message="user" key={chunk.id} className="group flex shrink-0 flex-col items-end gap-[3px]"><div className="max-w-[84%] whitespace-pre-wrap rounded-[12px_12px_3px_12px] border border-aurora-accent-pink/30 bg-[color-mix(in_srgb,var(--aurora-accent-pink)_12%,var(--aurora-control-surface))] px-3 py-[9px] text-[12.5px] leading-[1.6] text-aurora-text-primary">{message.text}</div><Reactions message={message} index={chunk.index} copiedIndex={copiedIndex} onRetry={onRetry} onCopy={onCopy} onEdit={onEdit}/></div>
      return <div data-phoenix-message="assistant" key={chunk.id} className="group flex min-w-0 shrink-0 gap-[9px]"><span className="grid size-[26px] shrink-0 place-items-center rounded-[9px] border border-aurora-accent-pink/40 bg-aurora-accent-pink/10 text-aurora-accent-pink">{mark}</span><div className="min-w-0 flex-1"><div className="whitespace-pre-wrap text-[12.5px] font-semibold leading-[1.68] text-aurora-text-primary">{message.text}</div><Reactions message={message} index={chunk.index} copiedIndex={copiedIndex} onRetry={onRetry} onCopy={onCopy}/></div></div>
    })}
  </>
}
