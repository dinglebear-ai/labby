import { X } from 'lucide-react'

export function ActiveFilterStrip({ filters, onClear }: { filters: Array<{ id: string; label: string; remove: () => void }>; onClear: () => void }) {
  if (!filters.length) return null
  return <section aria-label="Active filters" className="mt-3 flex min-w-0 items-center gap-2 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium px-2.5 py-[7px] shadow-[var(--aurora-shadow-medium)]">
    <span className="px-1 text-[10px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Filters</span>
    <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto">{filters.map(filter => <button key={filter.id} type="button" onClick={filter.remove} aria-label={`Remove ${filter.label} filter`} className="inline-flex h-[26px] shrink-0 items-center gap-1.5 rounded-[7px] border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 px-2 text-[11px] font-semibold text-aurora-accent-strong hover:bg-aurora-accent-primary/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><span className="max-w-48 truncate">{filter.label}</span><X aria-hidden="true" className="size-2.5"/></button>)}</div>
    <button type="button" onClick={onClear} className="h-[26px] shrink-0 rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-2.5 text-[11px] font-semibold text-aurora-accent-strong hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">Clear All</button>
  </section>
}
