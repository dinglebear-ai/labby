import type { ReactNode } from 'react'

export function WorkspaceStats({ label, stats }: { label: string; stats: Array<{ label: string; value: ReactNode; suffix: string; color: string }> }) {
  return <div aria-label={label} className="grid grid-cols-[repeat(auto-fit,minmax(140px,1fr))] gap-y-2.5 rounded-b-aurora-3 border-t border-aurora-border-subtle bg-[var(--gw0-0_30)] px-3.5 py-3">
    {stats.map(stat => <div key={stat.label} className="min-w-0 border-l border-[color-mix(in_srgb,var(--aurora-border-default)_40%,transparent)] px-3">
      <div className="text-[9.5px] font-bold uppercase tracking-[.12em] text-[#99b8cb]">{stat.label}</div>
      <div className="mt-[3px] flex items-baseline gap-1.5"><span className="font-display text-[21px] font-extrabold tracking-[-.01em]" style={{ color: stat.color }}>{stat.value}</span><span className="text-[10.5px] text-aurora-text-muted">{stat.suffix}</span></div>
    </div>)}
  </div>
}
