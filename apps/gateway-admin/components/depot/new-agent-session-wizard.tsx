'use client'

import { useState } from 'react'
import { Check, ChevronRight, CircleCheck, Layers3, Link2, Play } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { AlpineMark, CodexMark, DebianMark, UbuntuMark } from './brand-marks'

type WizardProps = { open: boolean; onOpenChange: (open: boolean) => void }
type Choice = { id: string; title: string; detail: string; icon: React.ReactNode }

const steps = [
  ['Loadout', 'project-a-loadout'],
  ['Container', 'platform-base'],
  ['Repository', 'tootie-tv/labby'],
  ['Harness', 'Claude Code'],
  ['Review', 'ready to start'],
] as const

const loadouts: Choice[] = [
  { id: 'project-a-loadout', title: 'project-a-loadout', detail: '10 artifacts · #project-a', icon: <Layers3 /> },
  { id: 'project-b-loadout', title: 'project-b-loadout', detail: '8 artifacts · #project-b', icon: <Layers3 /> },
  { id: 'platform-core', title: 'platform-core', detail: '6 artifacts · #platform', icon: <Layers3 /> },
  { id: 'oncall-loadout', title: 'oncall-loadout', detail: '5 artifacts · #support', icon: <Layers3 /> },
]
const containers: Choice[] = [
  { id: 'platform-base', title: 'platform-base', detail: 'Ubuntu 24.04 · 1.9 GB', icon: <UbuntuMark /> },
  { id: 'rust-heavy', title: 'rust-heavy', detail: 'Debian 12 · 2.4 GB', icon: <DebianMark /> },
  { id: 'edge-minimal', title: 'edge-minimal', detail: 'Alpine 3.21 · building', icon: <AlpineMark /> },
]
const harnesses: Choice[] = [
  { id: 'Claude Code', title: 'Claude Code', detail: 'coding harness', icon: <ClaudeMark /> },
  { id: 'Codex', title: 'Codex', detail: 'coding harness', icon: <CodexMark /> },
  { id: 'Gemini CLI', title: 'Gemini CLI', detail: 'coding harness', icon: <span className="text-xl leading-none">✦</span> },
]

function ClaudeMark(){return <span aria-label="Claude" className="text-[13px] font-black tracking-[-.16em]">AI</span>}

export function NewAgentSessionWizard({ open, onOpenChange }: WizardProps) {
  const [step, setStep] = useState(1)
  const [loadout, setLoadout] = useState('project-a-loadout')
  const [container, setContainer] = useState('platform-base')
  const [repository, setRepository] = useState('tootie-tv/labby')
  const [harness, setHarness] = useState('Claude Code')

  const values = [loadout, container, repository, harness, 'ready to start']
  const close = (next: boolean) => { onOpenChange(next); if (!next) window.setTimeout(() => setStep(1), 200) }
  const next = () => { if (step < 5) setStep((current) => current + 1); else close(false) }

  return <Dialog open={open} onOpenChange={close}>
    <DialogContent showCloseButton className="h-[min(660px,calc(100svh-2rem))] w-[min(1020px,calc(100vw-2rem))] max-w-none gap-0 border-aurora-border-strong bg-aurora-panel-medium p-0 shadow-aurora-panel sm:max-w-none">
      <DialogDescription className="sr-only">Choose a loadout, container, repository, and coding harness for the new agent session.</DialogDescription>
      <div className="grid min-h-0 flex-1 md:grid-cols-[214px_minmax(0,1fr)]">
        <aside className="border-r border-aurora-border-subtle bg-aurora-panel-low px-4 py-5">
          <p className="text-[9px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Agent session</p>
          <DialogTitle className="mt-2 text-base text-aurora-text-primary">New Session</DialogTitle>
          <nav aria-label="Session setup steps" className="mt-6 space-y-1.5">
            {steps.map(([label], index) => { const number=index+1; const complete=number<step; const active=number===step; return <button key={label} type="button" onClick={()=>setStep(number)} className={cn('flex w-full items-center gap-3 rounded-aurora-2 border px-2.5 py-2 text-left transition-colors',active?'border-aurora-accent-primary bg-aurora-selected-bg shadow-[inset_0_0_0_1px_var(--aurora-warn)]':'border-transparent hover:bg-aurora-hover-bg')}><span className={cn('grid size-6 shrink-0 place-items-center rounded-full border text-xs',complete?'border-aurora-success/50 bg-aurora-success/10 text-aurora-success':active?'border-aurora-accent-primary bg-aurora-accent-primary text-aurora-page-bg':'border-aurora-border-strong text-aurora-text-muted')}>{complete?<Check className="size-3.5"/>:number}</span><span className="min-w-0"><strong className="block text-xs text-aurora-text-primary">{label}</strong><span className="block truncate text-[10px] text-aurora-text-muted">{values[index]}</span></span></button> })}
          </nav>
        </aside>

        <section className="flex min-w-0 flex-col">
          <header className="flex h-14 items-center gap-3 border-b border-aurora-border-subtle px-4">
            <h2 className="text-base font-semibold text-aurora-text-primary">{steps[step-1][0]}</h2>
            <p className="text-xs text-aurora-text-muted">{['Which artifacts the session’s Labby comes up with.','The image the agent runs inside.','Cloned into the container before the agent starts.','The coding agent driving the session.','Confirm and provision.'][step-1]}</p>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto p-4">
            {step===1?<ChoiceGrid choices={loadouts} value={loadout} onChange={setLoadout}/>:null}
            {step===2?<ChoiceGrid choices={containers} value={container} onChange={setContainer}/>:null}
            {step===3?<RepositoryStep value={repository} onChange={setRepository}/>:null}
            {step===4?<ChoiceGrid choices={harnesses} value={harness} onChange={setHarness}/>:null}
            {step===5?<Review loadout={loadout} container={container} repository={repository} harness={harness}/>:null}
          </div>
          <footer className="flex h-14 items-center justify-between border-t border-aurora-border-subtle bg-aurora-panel-low px-4">
            <span className="text-[10px] text-aurora-text-muted">Step {step} of 5 · {loadout} · {container}</span>
            <div className="flex gap-2"><Button variant="outline" disabled={step===1} onClick={()=>setStep((current)=>Math.max(1,current-1))}>Back</Button><Button onClick={next} className={step===5?'bg-aurora-error text-aurora-page-bg hover:bg-aurora-error/90':''}>{step===5?<><Play/>Start Session</>:<>Next<ChevronRight/></>}</Button></div>
          </footer>
        </section>
      </div>
    </DialogContent>
  </Dialog>
}

function ChoiceGrid({ choices, value, onChange }: { choices: Choice[]; value: string; onChange: (value: string) => void }) {
  return <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">{choices.map((choice)=><button key={choice.id} type="button" aria-pressed={value===choice.id} onClick={()=>onChange(choice.id)} className="group flex min-h-14 items-center gap-3 rounded-aurora-2 border border-transparent bg-aurora-panel-low px-3 py-2 text-left transition-colors hover:border-aurora-accent-primary/40 aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-selected-bg"><span className="grid size-8 shrink-0 place-items-center rounded-aurora-1 bg-aurora-page-bg text-aurora-accent-primary [&_svg]:size-4">{choice.icon}</span><span className="min-w-0 flex-1"><strong className="block truncate text-xs text-aurora-text-primary">{choice.title}</strong><span className="block truncate text-[10px] text-aurora-text-muted">{choice.detail}</span></span>{value===choice.id?<CircleCheck className="size-4 shrink-0 fill-aurora-accent-primary text-aurora-page-bg"/>:null}</button>)}</div>
}

function RepositoryStep({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const recent=['tootie-tv/labby','tootie-tv/axon','tootie-tv/depot','tootie-tv/dotfiles']
  return <div className="space-y-3"><label className="flex h-11 items-center gap-3 rounded-aurora-2 border border-aurora-success/60 bg-aurora-page-bg/70 px-3"><Link2 className="size-4 text-aurora-text-muted"/><input aria-label="Repository" value={`github.com/${value}`} onChange={(event)=>onChange(event.target.value.replace(/^https?:\/\/github\.com\//,'').replace(/^github\.com\//,''))} className="min-w-0 flex-1 bg-transparent font-mono text-sm text-aurora-text-primary outline-none"/><Badge variant="outline" className="border-0 text-aurora-success"><span className="mr-1 size-1.5 rounded-full bg-aurora-success"/>Resolved</Badge></label><div><p className="mb-2 text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Recent</p><div className="flex flex-wrap gap-2">{recent.map((repo)=><button key={repo} type="button" onClick={()=>onChange(repo)} className="rounded-full border border-aurora-border-subtle bg-aurora-control-surface px-3 py-1.5 text-[10px] font-semibold text-aurora-text-muted hover:text-aurora-text-primary">{repo}</button>)}</div></div></div>
}

function Review({ loadout, container, repository, harness }: { loadout: string; container: string; repository: string; harness: string }) {
  return <div className="max-w-xl overflow-hidden rounded-aurora-3 border border-aurora-border-subtle bg-aurora-panel-low"><div className="flex items-center justify-between border-b border-aurora-border-subtle px-4 py-3"><span className="text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Review</span><span className="text-xs font-semibold text-aurora-success">Ready</span></div><dl>{[['Loadout',loadout],['Container',container],['Repository',repository],['Harness',harness]].map(([label,value])=><div key={label} className="grid grid-cols-[1fr_1fr] items-center border-b border-aurora-border-subtle px-4 py-3"><dt className="flex items-center gap-3 text-[9px] font-bold uppercase tracking-[.12em] text-aurora-text-muted"><span className="grid size-5 place-items-center rounded-full bg-aurora-success/15 text-aurora-success"><Check className="size-3"/></span>{label}</dt><dd className="text-right text-xs font-semibold text-aurora-text-primary">{value}</dd></div>)}</dl><p className="px-4 py-4 text-[10px] leading-5 text-aurora-text-muted">Provisions {container}, seeds Labby with {loadout}, clones the repo, then hands off.</p></div>
}
