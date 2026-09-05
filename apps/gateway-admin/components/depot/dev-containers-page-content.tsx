'use client'

import { useState } from 'react'
import { Check, ChevronLeft, ChevronRight, CirclePlus, Container, Grid2X2, List, Search, Table2 } from 'lucide-react'

import { ConsoleHero } from '@/components/console/console-hero'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { LocalBrandMark } from './brand-marks'

const containers = [
  ['Ready', 'platform-base', 'Ubuntu 24.04', 'Node · Python · Rust · Docker', '38 pulls'],
  ['Ready', 'rust-heavy', 'Debian 12', 'Rust · PostgreSQL · Docker', '12 pulls'],
  ['Building', 'edge-minimal', 'Alpine 3.21', 'Go · Docker · Tailscale', 'Layer 4/7'],
]

const steps = [
  ['Distro', 'Ubuntu 24.04'], ['Toolchains', '3 selected'], ['Agents', '2 harnesses'],
  ['Packages', '2 pinned'], ['Network', 'tailnet · web on'], ['Loadouts', '1 seeded'],
  ['Repos & Dotfiles', '1 repo'],
] as const

const options = {
  0: ['Ubuntu 24.04', 'Debian 12', 'Arch Linux', 'Fedora 41', 'Alpine 3.21', 'openSUSE'],
  1: ['Node.js', 'Python', 'Rust', 'Go', 'Bun', 'Deno', 'Docker', 'PostgreSQL', 'Ruby', 'PHP'],
  2: ['Claude Code', 'Codex', 'Gemini CLI', 'Copilot CLI', 'Aider', 'OpenCode'],
  3: ['ripgrep', 'fd-find', 'build-essential', 'jq', 'tmux', 'neovim'],
  5: ['project-a-loadout', 'project-b-loadout', 'platform-core', 'oncall-loadout'],
  6: ['tootie-tv/labby', 'tootie-tv/axon', 'tootie-tv/depot', 'tootie-tv/dotfiles'],
} as const

const initiallySelected: Record<number, string[]> = {
  0: ['Ubuntu 24.04'], 1: ['Node.js', 'Python', 'Rust'], 2: ['Claude Code', 'Codex'],
  3: ['ripgrep', 'fd-find'], 5: ['project-a-loadout'], 6: ['tootie-tv/labby'],
}

function ToggleRow({ label, detail, initial = true }: { label: string; detail: string; initial?: boolean }) {
  const [on, setOn] = useState(initial)
  return <button type="button" onClick={() => setOn(!on)} className="flex w-full items-center gap-4 rounded-aurora-1 bg-aurora-panel-low px-4 py-3 text-left">
    <span className={cn('relative h-5 w-9 rounded-full border transition-colors', on ? 'border-aurora-accent-primary bg-aurora-accent-primary' : 'border-aurora-border-strong bg-aurora-page-bg')}><span className={cn('absolute top-0.5 size-3.5 rounded-full bg-white transition-transform', on ? 'translate-x-[17px]' : 'translate-x-0.5')} /></span>
    <span><strong className="block text-sm text-aurora-text-primary">{label}</strong><span className="text-xs text-aurora-text-muted">{detail}</span></span>
  </button>
}

function ContainerWizard({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [step, setStep] = useState(0)
  const [selected, setSelected] = useState(initiallySelected)
  const toggle = (value: string) => setSelected((current) => {
    const values = current[step] ?? []
    const next = step === 0 ? [value] : values.includes(value) ? values.filter((item) => item !== value) : [...values, value]
    return { ...current, [step]: next }
  })
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent className="h-[100dvh] w-screen max-w-none gap-0 overflow-hidden rounded-none border-aurora-border-strong bg-aurora-panel-medium p-0 sm:h-[min(720px,calc(100vh-2rem))] sm:w-full sm:max-w-[1080px] sm:rounded-lg" showCloseButton>
      <DialogTitle className="sr-only">New container</DialogTitle><DialogDescription className="sr-only">Configure a reusable development container.</DialogDescription>
      <div className="grid min-h-0 flex-1 grid-rows-[auto_minmax(0,1fr)] sm:grid-cols-[230px_1fr] sm:grid-rows-1">
        <aside className="overflow-x-auto border-b border-aurora-border-default bg-aurora-panel-strong p-3 sm:overflow-visible sm:border-b-0 sm:border-r sm:p-4">
          <p className="text-[10px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Incus system container</p><h2 className="mt-1 text-lg font-semibold">New Container</h2>
          <ol className="mt-5 space-y-1">{steps.map(([label, detail], index) => <li key={label}><button type="button" onClick={() => setStep(index)} className={cn('flex w-full items-center gap-3 rounded-aurora-1 border px-2 py-2 text-left', step === index ? 'border-aurora-warn bg-aurora-selected-bg shadow-[0_0_0_1px_var(--aurora-warn)]' : 'border-transparent')}><span className={cn('grid size-6 shrink-0 place-items-center rounded-full border text-xs', index < step ? 'border-aurora-success/50 bg-aurora-success/10 text-aurora-success' : step === index ? 'border-aurora-accent-primary bg-aurora-accent-primary text-aurora-page-bg' : 'border-aurora-border-strong text-aurora-text-muted')}>{index < step ? <Check className="size-3"/> : index + 1}</span><span><strong className="block text-sm">{label}</strong><span className="block max-w-[145px] truncate text-[11px] text-aurora-text-muted">{detail}</span></span></button></li>)}</ol>
        </aside>
        <div className="flex min-h-0 flex-col">
          <header className="flex items-baseline gap-3 border-b border-aurora-border-default px-5 py-4"><h3 className="text-lg font-semibold">{steps[step][0]}</h3><p className="text-xs text-aurora-text-muted">{['The base image every toolchain layers onto.','Runtimes and CLIs baked into the image — pin a version or track latest.','Coding harnesses available inside the container.','Search registries and pin exactly what ships.','Reachability and isolation. Resource limits stay with whoever runs the container.','Seed the member’s Labby with the artifacts this project needs.','Repositories cloned in, and the dotfiles that shape the shell.'][step]}</p></header>
          <div className="min-h-0 flex-1 overflow-auto p-5">
            {step === 4 ? <div className="mx-auto max-w-2xl space-y-3"><ToggleRow label="Join the team tailnet" detail="Containers authenticate with an OAuth client — no auth key to paste or rotate."/><ToggleRow label="Outbound web access" detail="Registries, GitHub and package mirrors. Off means a fully sealed build."/><ToggleRow label="LAN access" detail="Reach hosts on the local subnet. Off by default for team images." initial={false}/><ToggleRow label="Nested Docker" detail="Runs dockerd inside the system container for compose-based workflows."/></div> : <>
              {step === 3 ? <div className="mb-3 flex items-center justify-between gap-3"><div className="flex gap-1">{['apt','npm','PyPI','Homebrew','cargo','GH Release'].map((item, index) => <Badge key={item} variant={index ? 'outline' : 'default'}>{item}</Badge>)}</div><label className="relative"><Search className="absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-muted"/><input aria-label="Search packages" className="h-8 rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg pl-8 text-xs" placeholder="Search apt…"/></label></div> : null}
              <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">{(options[step as keyof typeof options] ?? []).map((item) => { const active = (selected[step] ?? []).includes(item); return <button data-hovercard="1" type="button" key={item} onClick={() => toggle(item)} className={cn('flex min-h-14 items-center gap-3 rounded-aurora-1 border bg-aurora-panel-low px-3 text-left transition-[transform,border-color,box-shadow,background] active:translate-y-px', active ? 'border-aurora-accent-primary bg-aurora-selected-bg shadow-[0_0_0_1px_var(--aurora-accent-primary)]' : 'border-transparent hover:-translate-y-0.5 hover:border-aurora-border-strong')}><LocalBrandMark name={item}/><span className="min-w-0 flex-1"><strong className="block truncate text-sm">{item}</strong><small className="text-aurora-text-muted">{step === 0 ? 'base image' : step === 6 ? 'repository' : 'available'}</small></span>{active ? <span className="grid size-5 shrink-0 place-items-center rounded-full bg-aurora-accent-primary text-aurora-page-bg"><Check className="size-3"/></span> : null}</button>})}</div>
              {step === 6 ? <label className="mt-4 block rounded-aurora-1 bg-aurora-panel-low p-3 text-sm font-semibold">Dotfiles repository<input defaultValue="github.com/tootie-tv/dotfiles" className="mt-1 h-9 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg px-3 font-mono text-xs"/></label> : null}
            </>}
          </div>
          <footer className="flex items-center justify-between border-t border-aurora-border-default bg-aurora-panel-strong px-5 py-3"><span className="text-[11px] text-aurora-text-muted">{step < 6 ? `Step ${step + 1} of 7 · Ubuntu 24.04 · 3 toolchains · 2 agents` : 'Container creation is not connected to a runtime yet.'}</span><div className="flex gap-2"><Button variant="outline" disabled={step === 0} onClick={() => setStep(step - 1)}><ChevronLeft/>Back</Button>{step < 6 ? <Button variant="outline" onClick={() => setStep(step + 1)}>Next<ChevronRight/></Button> : <Button disabled title="Container creation is unavailable"><Container/>Build unavailable</Button>}</div></footer>
        </div>
      </div>
    </DialogContent>
  </Dialog>
}

export function DevContainersPageContent() {
  const [open, setOpen] = useState(false)
  const [view, setView] = useState<'table' | 'list' | 'cards'>('cards')
  const modes = [[Table2,'Table','table'],[List,'List','list'],[Grid2X2,'Cards','cards']] as const
  const cards = containers.map(([status, name, distro, tools, foot]) => (
    <section data-hovercard="1" key={name} className={view === 'cards' ? 'rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium p-5' : 'grid gap-4 px-4 py-4 md:grid-cols-[minmax(0,1fr)_180px_2fr_auto] md:items-center'}>
      <div className="flex justify-between gap-3"><h2 className="font-semibold text-aurora-text-primary">{name}</h2>{view === 'cards' ? <Badge variant="outline">{status}</Badge> : null}</div>
      <div className={view === 'cards' ? 'mt-4 flex items-center gap-3' : 'flex items-center gap-3'}><LocalBrandMark name={distro}/><p className="text-sm text-aurora-text-muted">{distro}</p></div>
      <div className={view === 'cards' ? 'mt-5 flex flex-wrap items-center gap-2 text-xs text-aurora-accent-primary' : 'flex flex-wrap items-center gap-2 text-xs text-aurora-accent-primary'}><Container size={15}/>{tools.split(' · ').map((tool) => <span key={tool} className="inline-flex items-center gap-1 rounded-aurora-1 bg-aurora-page-bg/50 px-2 py-1">{view === 'cards' ? <LocalBrandMark name={tool}/> : null}{tool}</span>)}</div>
      <div className={view === 'cards' ? 'mt-5 border-t border-aurora-border-subtle pt-3 text-xs text-aurora-text-muted' : 'text-xs text-aurora-text-muted'}>{view === 'list' ? <Badge variant="outline" className="mr-2">{status}</Badge> : null}{foot}</div>
    </section>
  ))
  return <>
    <ConsoleHero eyebrow="Workspace · Incus" title="Dev Containers" pulse={{color:'var(--aurora-success)'}} actions={<div className="flex items-center gap-2"><div className="flex rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5">{modes.map(([Icon,label,mode])=><button key={mode} type="button" aria-label={`${label} view`} title={`${label} view`} aria-pressed={view===mode} onClick={()=>setView(mode)} className="rounded p-1.5 text-aurora-text-muted hover:text-aurora-text-primary aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-primary"><Icon className="size-3.5"/></button>)}</div><Button onClick={() => setOpen(true)}><CirclePlus/>New container</Button></div>}/>
    {view === 'table' ? <div className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium"><table className="w-full text-left text-sm"><thead className="border-b border-aurora-border-subtle text-[10px] uppercase tracking-[.14em] text-aurora-text-muted"><tr><th className="px-4 py-3">Container</th><th>Distro</th><th>Toolchains</th><th>Status</th><th>Activity</th></tr></thead><tbody>{containers.map(([status,name,distro,tools,foot])=><tr key={name} className="border-b border-aurora-border-subtle/70 last:border-0 hover:bg-aurora-hover-bg"><td className="px-4 py-3 font-semibold text-aurora-text-primary">{name}</td><td><span className="flex items-center gap-2"><LocalBrandMark name={distro}/>{distro}</span></td><td className="text-aurora-text-muted">{tools}</td><td><Badge variant="outline">{status}</Badge></td><td className="text-aurora-text-muted">{foot}</td></tr>)}</tbody></table></div> : <div className={view === 'cards' ? 'grid gap-4 xl:grid-cols-3' : 'divide-y divide-aurora-border-subtle overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium'}>{cards}</div>}
    <ContainerWizard open={open} onOpenChange={setOpen}/>
  </>
}
