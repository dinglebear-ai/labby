import { cn } from '@/lib/utils'

export const DEFAULT_CONTAINER_NETWORK = { tailnet: true, web: true, lan: false, docker: true }
export type ContainerNetwork = typeof DEFAULT_CONTAINER_NETWORK

const settings = [
  ['tailnet', 'Join the team tailnet', 'Containers authenticate with an OAuth client — no auth key to paste or rotate.'],
  ['web', 'Outbound web access', 'Registries, GitHub and package mirrors. Off means a fully sealed build.'],
  ['lan', 'LAN access', 'Reach hosts on the local subnet. Off by default for team images.'],
  ['docker', 'Nested Docker', 'Runs dockerd inside the system container for compose-based workflows.'],
] as const

export function ContainerNetworkOptions({ value, onChange }: { value: ContainerNetwork; onChange: (value: ContainerNetwork) => void }) {
  return <div className="flex max-w-[620px] flex-col gap-2.5">
    {settings.map(([key, label, hint]) => <div key={key} className="flex min-w-0 items-center gap-3 rounded-[11px] border border-aurora-border-subtle bg-[var(--gw0-0_36)] px-[13px] py-3">
      <button type="button" role="switch" aria-checked={value[key]} aria-label={label} aria-describedby={`container-network-${key}-hint`} onClick={() => onChange({ ...value, [key]: !value[key] })} className={cn('relative h-[19px] w-[34px] shrink-0 rounded-full border transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary', value[key] ? 'border-aurora-accent-primary bg-aurora-accent-primary' : 'border-aurora-border-strong bg-aurora-control-surface')}>
        <span className={cn('absolute top-0.5 size-[13px] rounded-full bg-aurora-text-primary transition-[left]', value[key] ? 'left-[17px]' : 'left-0.5')}/>
      </button>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5"><span className="text-[12.5px] font-[650] text-aurora-text-primary">{label}</span><span id={`container-network-${key}-hint`} className="text-pretty text-[10.5px] leading-[1.45] text-aurora-text-muted">{hint}</span></span>
    </div>)}
  </div>
}
