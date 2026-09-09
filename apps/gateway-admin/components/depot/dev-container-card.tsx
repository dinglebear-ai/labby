import { Check, LoaderCircle, Cpu, MemoryStick, HardDrive, Download, Users, Folder, Network } from 'lucide-react'
import { LocalBrandMark } from './brand-marks'

/** Reference card composition; absent runtime measurements remain explicitly unknown. */
export function DevContainerCard({ row }: { row: string[] }) {
  const [status, name, distro, tools, activity] = row
  const building = status === 'Building'
  return <section data-hovercard="1" aria-label={name} className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-gradient-to-b from-aurora-panel-strong-top to-aurora-panel-strong shadow-aurora-medium">
    <header className="flex items-center gap-2 border-b border-aurora-border-subtle bg-[var(--gw0-0_38)] px-[15px] py-2.5">
      <span className="[&>span]:size-4 [&>span]:rounded-none [&>span]:bg-transparent [&>span]:p-0"><LocalBrandMark name={distro}/></span>
      <h2 className="min-w-0 flex-1 truncate font-display text-sm font-[760] text-aurora-text-primary">{name}</h2>
      <span aria-label={status} title={status} className={`grid size-[22px] place-items-center rounded-[7px] border border-current bg-aurora-control-surface ${building ? 'text-aurora-accent-strong' : 'text-aurora-success'}`}>
        {building ? <LoaderCircle className="size-3.5"/> : <Check className="size-3.5"/>}
      </span>
    </header>
    <div className="flex flex-col gap-2.5 px-[13px] py-3">
      <p className="text-[11.5px] text-aurora-text-muted">{distro}</p>
      {building && <p className="text-[10px] text-aurora-accent-strong">{activity}</p>}
      <div className="flex flex-col gap-1.5" aria-label="Runtime measurements unavailable">
        {[[Cpu, 'CPU'], [MemoryStick, 'Memory'], [HardDrive, 'Disk']].map(([Icon, label]) => {
          const MetricIcon = Icon as typeof Cpu
          return <div key={String(label)} className="flex items-center gap-2 text-aurora-text-muted"><MetricIcon aria-label={String(label)} className="size-3.5 shrink-0"/><span className="h-[5px] min-w-0 flex-1 rounded-full bg-aurora-page-bg"/><span className="w-[74px] text-right text-[10px] font-[650]">unavailable</span></div>
        })}
      </div>
      <div className="flex flex-col gap-[5px] text-[10px] text-aurora-text-muted">
        <span className="flex items-center gap-[7px]"><Folder className="size-3.5"/>Mounts unavailable</span>
        <span className="flex items-center gap-[7px]"><Network className="size-3.5"/>Ports unavailable</span>
      </div>
      <div className="flex flex-wrap gap-1.5" aria-label="Toolchains">
        {tools.split(' · ').map(tool => <span key={tool} title={tool} aria-label={tool} className="grid size-[26px] place-items-center rounded-lg border border-aurora-border-subtle bg-[var(--gw0-0_40)] [&>span]:size-4 [&>span]:rounded-none [&>span]:bg-transparent [&>span]:p-0"><LocalBrandMark name={tool === 'Node' ? 'Node.js' : tool}/></span>)}
      </div>
      <footer className="flex items-center gap-2 border-t border-aurora-border-subtle pt-[9px] text-[10.5px] font-[650] text-aurora-text-muted">
        <span className="flex items-center gap-[5px]" title="Recorded pulls"><Download className="size-[11px] text-aurora-accent-strong"/>{building ? '—' : activity.replace(' pulls', '')}</span>
        <span className="flex items-center gap-[5px]" title="Member count unavailable"><Users className="size-[11px] text-aurora-accent-pink"/>—</span>
      </footer>
    </div>
  </section>
}
