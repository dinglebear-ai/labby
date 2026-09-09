import { Check } from 'lucide-react'
import { cn } from '@/lib/utils'
import { LocalBrandMark } from './brand-marks'

export function ContainerOption({ name, detail, selected, onToggle }: { name: string; detail: string; selected: boolean; onToggle: () => void }) {
  return <button type="button" aria-pressed={selected} onClick={onToggle} className={cn(
    'flex min-w-0 items-center gap-2.5 rounded-[11px] border px-3 py-[11px] text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary',
    selected ? 'border-aurora-accent-primary bg-aurora-selected-bg' : 'border-aurora-border-subtle bg-aurora-control-surface hover:border-aurora-border-strong hover:bg-aurora-hover-bg',
  )}>
    <span className="grid size-[30px] shrink-0 place-items-center rounded-lg border border-aurora-border-subtle bg-[var(--gw0-0_45)] [&>span]:size-[18px] [&>span]:rounded-none [&>span]:bg-transparent [&>span]:p-0"><LocalBrandMark name={name}/></span>
    <span className="flex min-w-0 flex-1 flex-col gap-0.5"><strong className="truncate pr-0.5 text-[12.5px] font-[650] text-aurora-text-primary">{name}</strong><span className="truncate pr-0.5 text-[10px] text-aurora-text-muted">{detail}</span></span>
    {selected && <span className="grid size-[17px] shrink-0 place-items-center rounded-full bg-aurora-accent-primary text-aurora-page-bg"><Check className="size-2.5"/></span>}
  </button>
}
