'use client'

import { useState } from 'react'
import { Check, Search } from 'lucide-react'

import { DevContainersHero } from './dev-containers-hero'
import { DevContainerCard } from './dev-container-card'
import { ContainerOption } from './container-option'
import { ContainerNetworkOptions, DEFAULT_CONTAINER_NETWORK } from './container-network'
import { ContainerVersionPins } from './container-version-pins'
import { ContainerWizardFooter } from './container-wizard-footer'
import { CONTAINER_OPTION_DETAILS } from './container-option-details'
import { Badge } from '@/components/ui/badge'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'

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


function ContainerWizard({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [step, setStep] = useState(0)
  const [selected, setSelected] = useState(initiallySelected)
  const [network, setNetwork] = useState(DEFAULT_CONTAINER_NETWORK)
  // Draft defaults from the visual reference, not installed/runtime versions.
  const [versions, setVersions] = useState<Record<string, string>>({ 'Node.js': '22.11.0', Python: '3.12.7', Rust: '1.83.0', ripgrep: '14.1.1', 'fd-find': '10.2.0' })
  const summary = (index: number, fallback: string) => {
    if (index === 4) return `${network.tailnet ? 'tailnet' : 'no tailnet'} · web ${network.web ? 'on' : 'off'}`
    const values = selected[index]
    if (!values) return fallback
    if (index === 0) return values[0] ?? 'Choose a distro'
    const units: Record<number, string> = { 1: 'selected', 2: 'harnesses', 3: 'selected', 5: 'seeded', 6: values.length === 1 ? 'repo' : 'repos' }
    return `${values.length} ${units[index]}`
  }
  const toggle = (value: string) => setSelected((current) => {
    const values = current[step] ?? []
    const next = step === 0 ? [value] : values.includes(value) ? values.filter((item) => item !== value) : [...values, value]
    return { ...current, [step]: next }
  })
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent className="h-[100dvh] w-screen max-w-none gap-0 overflow-hidden rounded-none border-aurora-border-strong bg-aurora-panel-medium p-0 sm:h-[min(720px,calc(100vh-2rem))] sm:w-full sm:max-w-[1080px] sm:rounded-lg" showCloseButton>
      <DialogTitle className="sr-only">New container</DialogTitle><DialogDescription className="sr-only">Configure a reusable development container.</DialogDescription>
      <div className="grid min-h-0 flex-1 grid-rows-[auto_minmax(0,1fr)] sm:grid-cols-[230px_1fr] sm:grid-rows-1">
        <aside className="aurora-scrollbar overflow-x-auto border-b border-aurora-border-default bg-aurora-panel-strong p-3 sm:overflow-visible sm:border-b-0 sm:border-r sm:p-4">
          <p className="text-[10px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Incus system container</p><h2 className="mt-1 text-lg font-semibold">New Container</h2>
          <ol className="mt-3 flex w-max gap-1 sm:mt-5 sm:block sm:w-auto sm:space-y-1">{steps.map(([label, detail], index) => <li key={label}><button type="button" onClick={() => setStep(index)} className={cn('flex w-[168px] items-center gap-3 sm:w-full rounded-aurora-1 border px-2 py-2 text-left', step === index ? 'border-aurora-warn bg-aurora-selected-bg shadow-[0_0_0_1px_var(--aurora-warn)]' : 'border-transparent')}><span className={cn('grid size-6 shrink-0 place-items-center rounded-full border text-xs', index < step ? 'border-aurora-success/50 bg-aurora-success/10 text-aurora-success' : step === index ? 'border-aurora-accent-primary bg-aurora-accent-primary text-aurora-page-bg' : 'border-aurora-border-strong text-aurora-text-muted')}>{index < step ? <Check className="size-3"/> : index + 1}</span><span><strong className="block text-sm">{label}</strong><span className="block max-w-[145px] truncate text-[11px] text-aurora-text-muted">{summary(index, detail)}</span></span></button></li>)}</ol>
        </aside>
        <div className="flex min-h-0 flex-col">
          <header className="flex items-baseline gap-3 border-b border-aurora-border-default px-5 py-4"><h3 className="text-lg font-semibold">{steps[step][0]}</h3><p className="text-xs text-aurora-text-muted">{['The base image every toolchain layers onto.','Runtimes and CLIs baked into the image — pin a version or track latest.','Coding harnesses available inside the container.','Search registries and pin exactly what ships.','Reachability and isolation. Resource limits stay with whoever runs the container.','Seed the member’s Labby with the artifacts this project needs.','Repositories cloned in, and the dotfiles that shape the shell.'][step]}</p></header>
          <div className="aurora-scrollbar min-h-0 flex-1 overflow-auto p-5">
            {step === 4 ? <ContainerNetworkOptions value={network} onChange={setNetwork}/> : <>
              {step === 3 ? <div className="mb-3 flex items-center justify-between gap-3"><div className="flex gap-1">{['apt','npm','PyPI','Homebrew','cargo','GH Release'].map((item, index) => <Badge key={item} variant={index ? 'outline' : 'default'}>{item}</Badge>)}</div><label className="relative"><Search className="absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-muted"/><input aria-label="Search packages" className="h-8 rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg pl-8 text-xs" placeholder="Search apt…"/></label></div> : null}
              <div className="grid grid-cols-[repeat(auto-fill,minmax(min(196px,100%),1fr))] gap-2.5">{(options[step as keyof typeof options] ?? []).map(item => <ContainerOption key={item} name={item} detail={CONTAINER_OPTION_DETAILS[item] ?? (step === 6 ? 'repository' : 'available')} selected={(selected[step] ?? []).includes(item)} onToggle={() => toggle(item)}/>)}</div>
              {(step === 1 || step === 3) && <ContainerVersionPins names={selected[step] ?? []} versions={versions} onVersion={(name, version) => setVersions(current => ({ ...current, [name]: version }))} onRemove={toggle}/>}
              {step === 6 ? <label className="mt-4 block rounded-aurora-1 bg-aurora-panel-low p-3 text-sm font-semibold">Dotfiles repository<input defaultValue="github.com/tootie-tv/dotfiles" className="mt-1 h-9 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg px-3 font-mono text-xs"/></label> : null}
            </>}
          </div>
          <ContainerWizardFooter step={step} onStep={setStep} summary={`Step ${step + 1} of 7 · ${selected[0]?.[0] ?? 'No distro'} · ${selected[1]?.length ?? 0} toolchains · ${selected[2]?.length ?? 0} agents`}/>
        </div>
      </div>
    </DialogContent>
  </Dialog>
}
export function DevContainersPageContent() {
  const [open, setOpen] = useState(false)
  return <>
    <DevContainersHero rows={containers} onCreate={() => setOpen(true)}/>
    <div aria-label="Container images" className="grid grid-cols-[repeat(auto-fill,minmax(min(310px,100%),1fr))] items-start gap-3">
      {containers.map(row => <DevContainerCard key={row[1]} row={row}/>)}
    </div>
    <ContainerWizard open={open} onOpenChange={setOpen}/>
  </>
}
