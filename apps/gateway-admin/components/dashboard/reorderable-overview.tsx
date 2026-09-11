'use client'

import { useEffect, useState, type ReactNode } from 'react'
import { ArrowDown, ArrowUp, GripVertical, Maximize, Minimize } from 'lucide-react'
import { cn } from '@/lib/utils'

export function normalizeOverviewOrder(saved: unknown, ids: string[]): string[] {
  const retained = Array.isArray(saved) ? saved.filter((id): id is string => typeof id === 'string' && ids.includes(id)) : []
  return [...new Set([...retained, ...ids])]
}

const OVERVIEW_ORDER_KEY = 'labby:overview-card-order:v1'
const OVERVIEW_WIDTH_KEY = 'labby:overview-card-widths:v1'

export function normalizeOverviewWidths(saved: unknown, ids: string[]): Record<string, boolean> {
  if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return {}
  return Object.fromEntries(Object.entries(saved).filter(([id, value]) => ids.includes(id) && typeof value === 'boolean'))
}

export function ReorderableOverview({ cards }: { cards: Array<{ id: string; content: ReactNode; wide?: boolean; rail?: boolean }> }) {
  const ids = cards.map((card) => card.id)
  const [order, setOrder] = useState(ids)
  const [dragging, setDragging] = useState<string | null>(null)
  const [storageWarning, setStorageWarning] = useState(false)
  const [widths, setWidths] = useState<Record<string, boolean>>({})

  useEffect(() => {
    try {
      const saved: unknown = JSON.parse(window.localStorage.getItem(OVERVIEW_ORDER_KEY) ?? '[]')
      setOrder(normalizeOverviewOrder(saved, ids))
    } catch {
      setStorageWarning(true)
    }
    try {
      setWidths(normalizeOverviewWidths(JSON.parse(window.localStorage.getItem(OVERVIEW_WIDTH_KEY) ?? '{}'), cards.filter(card => !card.rail).map(card => card.id)))
    } catch {
      setStorageWarning(true)
    }
  // The card identities are stable for the lifetime of this dashboard.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const commit = (next: string[]) => {
    setOrder(next)
    try {
      window.localStorage.setItem(OVERVIEW_ORDER_KEY, JSON.stringify(next))
    } catch {
      setStorageWarning(true)
    }
  }
  const toggleWidth = (id: string, current: boolean) => {
    const next = { ...widths, [id]: !current }
    setWidths(next)
    try {
      window.localStorage.setItem(OVERVIEW_WIDTH_KEY, JSON.stringify(next))
    } catch {
      setStorageWarning(true)
    }
  }
  const move = (id: string, delta: number) => {
    const rail = cards.find(card => card.id === id)?.rail
    const lane = order.filter(item => Boolean(cards.find(card => card.id === item)?.rail) === Boolean(rail))
    const from = lane.indexOf(id)
    const to = Math.max(0, Math.min(lane.length - 1, from + delta))
    if (from === to) return
    const next = [...order]
    const source = next.indexOf(id)
    const target = next.indexOf(lane[to])
    ;[next[source], next[target]] = [next[target], next[source]]
    commit(next)
  }
  const dropOn = (id: string) => {
    if (!dragging || dragging === id) return
    if (Boolean(cards.find(card => card.id === id)?.rail) !== Boolean(cards.find(card => card.id === dragging)?.rail)) return
    const next = order.filter((item) => item !== dragging)
    next.splice(next.indexOf(id), 0, dragging)
    commit(next)
    setDragging(null)
  }

  return <section aria-label="Customizable overview cards">
    <p className="mb-2 flex items-center gap-1.5 text-[10px] text-aurora-text-muted"><GripVertical className="size-3"/>{storageWarning ? 'Layout changes work for this session, but this device could not read or save them.' : 'Drag cards to arrange your overview. The order is saved on this device.'}</p>
    <div data-overview-columns className="grid min-w-0 items-start gap-3 min-[1100px]:grid-cols-[minmax(0,2fr)_minmax(260px,1fr)]">
      {[false, true].map(rail => <div key={String(rail)} data-overview-lane={rail ? 'insights' : 'telemetry'} className={cn('grid min-w-0 items-start gap-3', !rail && 'min-[700px]:grid-cols-2')}>
      {order.filter(id => Boolean(cards.find(card => card.id === id)?.rail) === rail).map((id) => {
        const card = cards.find((item) => item.id === id)
        if (!card) return null
        const wide = widths[id] ?? Boolean(card.wide)
        return <div key={id} draggable onDragStart={() => setDragging(id)} onDragEnd={() => setDragging(null)} onDragOver={(event) => { if (Boolean(cards.find(card => card.id === dragging)?.rail) === rail) event.preventDefault() }} onDrop={() => dropOn(id)} className={cn('group relative min-w-0 cursor-grab rounded-aurora-2 outline-none active:cursor-grabbing', wide && !rail && 'min-[700px]:col-span-2', dragging === id && 'opacity-50')}>
          <div className="absolute right-3 top-2 z-10 flex items-center gap-0.5 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-strong/95 p-0.5 opacity-0 shadow-lg transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
            <GripVertical className="mx-1 size-3.5 text-aurora-text-muted" aria-hidden="true"/>
            <button type="button" onClick={() => move(id, -1)} className="rounded p-1 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" aria-label={`Move ${id} earlier`}><ArrowUp className="size-3"/></button>
            <button type="button" onClick={() => move(id, 1)} className="rounded p-1 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" aria-label={`Move ${id} later`}><ArrowDown className="size-3"/></button>
            {!rail && <button type="button" onClick={() => toggleWidth(id, wide)} aria-label={`Toggle ${id} full width`} aria-pressed={wide} title="Toggle full-width" className="rounded p-1 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-accent-strong">{wide ? <Minimize className="size-3"/> : <Maximize className="size-3"/>}</button>}
          </div>
          {card.content}
        </div>
      })}
      </div>)}
    </div>
  </section>
}
