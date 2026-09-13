'use client'

import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react'
import { ArrowDown, ArrowLeftRight, ArrowUp, GripVertical, Maximize, Minimize } from 'lucide-react'
import { cn } from '@/lib/utils'

type Lane = 'telemetry' | 'insights'
type Card = { id: string; content: ReactNode; wide?: boolean; rail?: boolean }
export type OverviewLayout = { order: string[]; widths: Record<string, boolean>; lanes: Record<string, Lane> }
type DropTarget = { id: string | null; lane: Lane; after: boolean }
type Drag = { id: string; x: number; y: number }
type PendingDrag = { id: string; x: number; y: number; pointerId: number; active: boolean; target: HTMLButtonElement }
const LAYOUT_KEY = 'labby:overview-layout:v2'

function releasePointer(source: PendingDrag) {
  try {
    if (source.target.hasPointerCapture(source.pointerId)) source.target.releasePointerCapture(source.pointerId)
  } catch {
    // Pointer cancellation or element removal may have already released capture.
  }
}

export function nearestOverviewDrop(
  cards: ReadonlyArray<{ id: string; left: number; right: number; top: number; bottom: number }>,
  x: number,
  y: number,
): { id: string | null; after: boolean } {
  if (cards.length === 0 || y >= Math.max(...cards.map(card => card.bottom))) return { id: null, after: true }
  let nearest: { id: string; after: boolean; distance: number } | null = null
  for (const card of cards) {
    const horizontalDistance = x < card.left ? card.left - x : x > card.right ? x - card.right : 0
    for (const [after, edge] of [[false, card.top], [true, card.bottom]] as const) {
      const distance = Math.hypot(horizontalDistance, y - edge)
      if (!nearest || distance < nearest.distance) nearest = { id: card.id, after, distance }
    }
  }
  return nearest ? { id: nearest.id, after: nearest.after } : { id: null, after: true }
}

export function normalizeOverviewOrder(saved: unknown, ids: string[]): string[] {
  const retained = Array.isArray(saved) ? saved.filter((id): id is string => typeof id === 'string' && ids.includes(id)) : []
  return [...new Set([...retained, ...ids])]
}

export function normalizeOverviewWidths(saved: unknown, ids: string[]): Record<string, boolean> {
  if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return {}
  return Object.fromEntries(Object.entries(saved).filter(([id, value]) => ids.includes(id) && typeof value === 'boolean'))
}

export function normalizeOverviewLayout(saved: unknown, ids: string[]): OverviewLayout {
  const value = saved && typeof saved === 'object' && !Array.isArray(saved) ? saved as Partial<OverviewLayout> : {}
  const lanes = value.lanes && typeof value.lanes === 'object' && !Array.isArray(value.lanes)
    ? Object.fromEntries(Object.entries(value.lanes).filter(([id, lane]) => ids.includes(id) && (lane === 'telemetry' || lane === 'insights'))) : {}
  return { order: normalizeOverviewOrder(value.order, ids), widths: normalizeOverviewWidths(value.widths, ids), lanes }
}

/** Remove first, then insert at the indicated edge: downward moves cannot lose an index. */
export function placeOverviewCard(layout: OverviewLayout, id: string, target: DropTarget): OverviewLayout {
  if (!layout.order.includes(id) || target.id === id || (target.id !== null && !layout.order.includes(target.id))) return layout
  const order = layout.order.filter(item => item !== id)
  const index = target.id === null ? order.length : order.indexOf(target.id) + Number(target.after)
  order.splice(index, 0, id)
  return { ...layout, order, lanes: { ...layout.lanes, [id]: target.lane } }
}

export function ReorderableOverview({ cards }: { cards: Card[] }) {
  const [layout, setLayout] = useState<OverviewLayout>(() => ({ order: cards.map(card => card.id), widths: {}, lanes: {} }))
  const layoutRef = useRef(layout)
  const root = useRef<HTMLElement>(null)
  const pending = useRef<PendingDrag | null>(null)
  const pointer = useRef({ x: 0, y: 0 })
  const animation = useRef<number | null>(null)
  const handles = useRef(new Map<string, HTMLButtonElement>())
  const focusAfterMove = useRef<string | null>(null)
  const [drag, setDrag] = useState<Drag | null>(null)
  const [drop, setDrop] = useState<DropTarget | null>(null)
  const [announcement, setAnnouncement] = useState('')
  const [storageWarning, setStorageWarning] = useState(false)
  const laneOf = (id: string) => layout.lanes[id] ?? (cards.find(card => card.id === id)?.rail ? 'insights' : 'telemetry')

  useEffect(() => {
    try {
      const saved = window.localStorage.getItem(LAYOUT_KEY)
      const next = normalizeOverviewLayout(saved ? JSON.parse(saved) : {
        order: JSON.parse(window.localStorage.getItem('labby:overview-card-order:v1') ?? '[]'),
        widths: JSON.parse(window.localStorage.getItem('labby:overview-card-widths:v1') ?? '{}'),
      }, cards.map(card => card.id))
      layoutRef.current = next
      setLayout(next)
    } catch { setStorageWarning(true) }
    return () => {
      const source = pending.current
      pending.current = null
      if (source) releasePointer(source)
      if (animation.current !== null) cancelAnimationFrame(animation.current)
    }
    // Card identities are fixed for this page; metrics updates must not reset a layout.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useLayoutEffect(() => {
    const id = focusAfterMove.current
    if (!id) return
    focusAfterMove.current = null
    handles.current.get(id)?.focus()
  }, [layout])

  const commit = (next: OverviewLayout, message: string, restoreFocusTo?: string) => {
    focusAfterMove.current = restoreFocusTo ?? null
    layoutRef.current = next
    setLayout(next)
    setAnnouncement(message)
    try { window.localStorage.setItem(LAYOUT_KEY, JSON.stringify(next)) } catch { setStorageWarning(true) }
  }
  const clearDrag = (releaseCapture = true) => {
    const source = pending.current
    pending.current = null
    setDrag(null)
    setDrop(null)
    if (animation.current !== null) cancelAnimationFrame(animation.current)
    animation.current = null
    if (releaseCapture && source) releasePointer(source)
  }
  const findDrop = (x: number, y: number): DropTarget | null => {
    const element = document.elementsFromPoint(x, y).find(item => root.current?.contains(item))
    const laneElement = element?.closest<HTMLElement>('[data-overview-lane]')
    if (!laneElement) return null
    const lane = laneElement.dataset.overviewLane as Lane
    const card = element?.closest<HTMLElement>('[data-overview-card]')
    if (!card) {
      const sourceId = pending.current?.id
      const cards = [...laneElement.querySelectorAll<HTMLElement>(':scope > [data-overview-card]')]
        .filter(item => item.dataset.overviewCard !== sourceId)
        .map(item => {
          const bounds = item.getBoundingClientRect()
          return { id: item.dataset.overviewCard!, left: bounds.left, right: bounds.right, top: bounds.top, bottom: bounds.bottom }
        })
      return { ...nearestOverviewDrop(cards, x, y), lane }
    }
    const bounds = card.getBoundingClientRect()
    return { id: card.dataset.overviewCard!, lane, after: y >= bounds.top + bounds.height / 2 }
  }
  const updateDrop = () => {
    const next = findDrop(pointer.current.x, pointer.current.y)
    setDrop(previous => previous?.id === next?.id && previous?.lane === next?.lane && previous?.after === next?.after ? previous : next)
  }
  const autoScroll = () => {
    if (!pending.current?.active) return
    let container = root.current?.parentElement
    while (container && !(container.scrollHeight > container.clientHeight && /auto|scroll/.test(getComputedStyle(container).overflowY))) container = container.parentElement
    const scroller = container ?? document.scrollingElement
    if (scroller) {
      const rect = container?.getBoundingClientRect()
      const top = Math.max(0, rect?.top ?? 0)
      const bottom = Math.min(window.innerHeight, rect?.bottom ?? window.innerHeight)
      const y = pointer.current.y
      const speed = y < top + 64 ? -Math.min(16, (top + 64 - y) / 4) : y > bottom - 64 ? Math.min(16, (y - bottom + 64) / 4) : 0
      if (speed) { scroller.scrollTop += speed; updateDrop() }
    }
    animation.current = requestAnimationFrame(autoScroll)
  }
  const pointerDown = (event: ReactPointerEvent<HTMLButtonElement>, id: string) => {
    if (event.button !== 0 || pending.current) return
    pending.current = { id, x: event.clientX, y: event.clientY, pointerId: event.pointerId, active: false, target: event.currentTarget }
    pointer.current = { x: event.clientX, y: event.clientY }
    try {
      event.currentTarget.setPointerCapture(event.pointerId)
    } catch {
      clearDrag()
      setAnnouncement(`Could not start pointer drag for ${id}. Use the move controls instead.`)
    }
  }
  const pointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const source = pending.current
    if (!source || source.pointerId !== event.pointerId) return
    pointer.current = { x: event.clientX, y: event.clientY }
    if (!source.active && Math.hypot(event.clientX - source.x, event.clientY - source.y) < 6) return
    event.preventDefault()
    if (!source.active) {
      source.active = true
      setAnnouncement(`Moving ${source.id}. Drop at an insertion line or press Escape to cancel.`)
      animation.current = requestAnimationFrame(autoScroll)
    }
    setDrag({ id: source.id, ...pointer.current })
    updateDrop()
  }
  const pointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const source = pending.current
    if (!source || source.pointerId !== event.pointerId) return
    // Compute from release coordinates too, including after autoscrolling.
    const target = findDrop(event.clientX, event.clientY)
    if (source.active && target && target.id !== source.id) commit(placeOverviewCard(layoutRef.current, source.id, target), `${source.id} moved to ${target.lane}.`)
    clearDrag()
  }
  const move = (id: string, delta: number) => {
    const lane = laneOf(id)
    const items = layout.order.filter(item => laneOf(item) === lane)
    const target = items[items.indexOf(id) + delta]
    if (target) commit(placeOverviewCard(layout, id, { id: target, lane, after: delta > 0 }), `${id} moved ${delta > 0 ? 'later' : 'earlier'}.`, id)
  }
  const controlClass = 'rounded p-1 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary disabled:opacity-30'

  return <section ref={root} aria-label="Customizable overview cards" onKeyDown={event => { if (event.key === 'Escape' && pending.current) { clearDrag(); setAnnouncement('Card move cancelled.') } }}>
    <p className={storageWarning ? 'mb-2 text-[10px] text-aurora-text-muted' : 'sr-only'}>{storageWarning ? 'Layout changes work for this session, but this device could not read or save them.' : 'Use a card’s drag handle to arrange your overview. Arrow buttons move within a column; the column button moves between columns. Layout is saved on this device.'}</p>
    <span role="status" aria-live="polite" className="sr-only">{announcement}</span>
    <div data-overview-columns className="grid min-w-0 items-start gap-3 min-[1100px]:grid-cols-[minmax(0,2fr)_minmax(260px,1fr)]">
      {(['telemetry', 'insights'] as const).map(lane => {
        const laneIds = layout.order.filter(id => laneOf(id) === lane)
        return <div key={lane} data-overview-lane={lane} className={cn('grid min-h-16 min-w-0 content-start items-start gap-3 self-stretch rounded-aurora-2', lane === 'telemetry' && 'min-[700px]:grid-cols-2', drag && drop?.lane === lane && drop.id === null && 'ring-2 ring-aurora-accent-primary')}>
          {laneIds.map((id, index) => {
            const card = cards.find(item => item.id === id)
            if (!card) return null
            const wide = layout.widths[id] ?? Boolean(card.wide)
            const targeted = drag && drag.id !== id && drop?.id === id
            return <div key={id} data-overview-card={id} className={cn('group relative min-w-0 rounded-aurora-2', wide && lane === 'telemetry' && 'min-[700px]:col-span-2', drag?.id === id && 'opacity-40')}>
              {targeted ? <div data-overview-insertion={drop.after ? 'after' : 'before'} aria-hidden className={cn('pointer-events-none absolute -inset-x-0.5 z-20 h-[3px] rounded-full bg-aurora-accent-primary shadow-[0_0_8px_var(--aurora-accent-primary)]', drop.after ? '-bottom-2' : '-top-2')}/> : null}
              <div className="absolute right-3 top-2 z-10 flex items-center gap-0.5 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-strong/95 p-0.5 opacity-0 shadow-lg transition-opacity group-hover:opacity-100 group-focus-within:opacity-100 max-md:opacity-100">
                <button ref={node => { if (node) handles.current.set(id, node); else handles.current.delete(id) }} type="button" aria-label={`Drag ${id}`} title="Drag to rearrange; arrow keys move within this column" className={cn(controlClass, 'touch-none cursor-grab active:cursor-grabbing')} onPointerDown={event => pointerDown(event, id)} onPointerMove={pointerMove} onPointerUp={pointerUp} onPointerCancel={() => clearDrag()} onLostPointerCapture={event => { if (pending.current?.pointerId === event.pointerId) clearDrag(false) }} onKeyDown={event => { if (event.key === 'ArrowUp' || event.key === 'ArrowDown') { event.preventDefault(); move(id, event.key === 'ArrowDown' ? 1 : -1) } }}><GripVertical className="size-3.5"/></button>
                <button type="button" disabled={index === 0} onClick={() => move(id, -1)} className={controlClass} aria-label={`Move ${id} earlier`}><ArrowUp className="size-3"/></button>
                <button type="button" disabled={index === laneIds.length - 1} onClick={() => move(id, 1)} className={controlClass} aria-label={`Move ${id} later`}><ArrowDown className="size-3"/></button>
                <button type="button" onClick={() => commit(placeOverviewCard(layout, id, { id: null, lane: lane === 'telemetry' ? 'insights' : 'telemetry', after: true }), `${id} moved to the other column.`, id)} className={controlClass} aria-label={`Move ${id} to ${lane === 'telemetry' ? 'insights' : 'telemetry'} column`}><ArrowLeftRight className="size-3"/></button>
                {lane === 'telemetry' && <button type="button" onClick={() => commit({ ...layout, widths: { ...layout.widths, [id]: !wide } }, `${id} is now ${wide ? 'half' : 'full'} width.`)} aria-label={`Toggle ${id} full width`} aria-pressed={wide} title="Toggle full-width" className={controlClass}>{wide ? <Minimize className="size-3"/> : <Maximize className="size-3"/>}</button>}
              </div>
              {card.content}
            </div>
          })}
        </div>
      })}
    </div>
    {drag ? <div aria-hidden className="pointer-events-none fixed z-50 max-w-56 truncate rounded-aurora-1 border border-aurora-accent-primary bg-aurora-panel-strong px-3 py-2 text-xs font-semibold text-aurora-text-primary shadow-lg" style={{ left: drag.x + 14, top: drag.y + 14 }}>{drag.id}</div> : null}
  </section>
}
