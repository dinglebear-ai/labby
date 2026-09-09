import { X } from 'lucide-react'
import { LocalBrandMark } from './brand-marks'

export function ContainerVersionPins({ names, versions, onVersion, onRemove }: { names: string[]; versions: Record<string, string>; onVersion: (name: string, version: string) => void; onRemove: (name: string) => void }) {
  if (!names.length) return null
  return <section aria-label="Selected pinned versions" className="mt-3.5 overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-gradient-to-b from-aurora-panel-strong-top to-aurora-panel-strong shadow-aurora-medium">
    <header className="flex items-center gap-2 border-b border-aurora-border-subtle bg-[var(--gw0-0_38)] px-[15px] py-2.5"><h4 className="flex-1 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted">Selected · pinned versions</h4><span className="text-[11px] font-[650] text-aurora-text-muted">{names.length} selected</span></header>
    {names.map(name => <div key={name} className="flex min-w-0 items-center gap-2.5 border-t border-aurora-border-subtle px-[15px] py-[9px] first-of-type:border-t-0">
      <span className="[&>span]:size-3.5 [&>span]:rounded-none [&>span]:bg-transparent [&>span]:p-0"><LocalBrandMark name={name}/></span>
      <span className="min-w-0 flex-1 truncate pr-0.5 text-xs font-semibold text-aurora-text-primary">{name}</span>
      <input aria-label={`Pin version for ${name}`} value={versions[name] ?? 'latest'} onChange={event => onVersion(name, event.target.value)} spellCheck={false} className="h-[26px] w-24 shrink-0 rounded-[7px] border border-aurora-border-strong bg-aurora-control-surface px-[9px] font-mono text-[11px] text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"/>
      <button type="button" aria-label={`Remove ${name}`} title={`Remove ${name}`} onClick={() => onRemove(name)} className="grid size-[22px] shrink-0 place-items-center rounded-md text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-error focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><X className="size-[11px]"/></button>
    </div>)}
  </section>
}
