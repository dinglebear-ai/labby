'use client'

import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react'
import { AlertCircle, CheckCircle2, Cpu, Play } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { createAgentFromHarness, runAgent, type AgentHarnessView, type AgentRunResult, type AgentView } from '@/lib/agent-tasks/client'

const EMPTY_LOADOUT_DIGEST = 'sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945'
const META = 'grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-space-4 text-[11px]'

function shortDigest(value: string) {
  return value.length > 30 ? `${value.slice(0, 22)}…${value.slice(-8)}` : value
}

export function NewAgentSessionWizard({
  open,
  onOpenChange,
  agents,
  harnesses,
  initialAgentId,
  canCreate = true,
  onStarted,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  agents: AgentView[]
  harnesses: AgentHarnessView[]
  initialAgentId?: string
  canCreate?: boolean
  onStarted: (session: AgentRunResult) => void
}) {
  const availableHarnesses = useMemo(() => harnesses.filter(harness => harness.available), [harnesses])
  const eligible = useMemo(() => agents.filter(agent => agent.state === 'active' && availableHarnesses.some(harness => harness.id === agent.harness_id && harness.digest === agent.harness_digest)), [agents, availableHarnesses])
  const [agentId, setAgentId] = useState('')
  const [newAgentId, setNewAgentId] = useState('')
  const [harnessId, setHarnessId] = useState('')
  const [createdAgent, setCreatedAgent] = useState<AgentView>()
  const [input, setInput] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string>()
  const pendingStart = useRef<{ intent: string; key: string } | undefined>(undefined)

  useEffect(() => {
    if (!open) return
    const requested = eligible.find(agent => agent.agent_id === initialAgentId)?.agent_id
    setAgentId(requested ?? eligible[0]?.agent_id ?? '')
    setNewAgentId('')
    setHarnessId(availableHarnesses[0]?.id ?? '')
    setCreatedAgent(undefined)
    setInput('')
    setError(undefined)
    pendingStart.current = undefined
  }, [availableHarnesses, eligible, initialAgentId, open])

  const createNew = canCreate && eligible.length === 0 && !createdAgent
  const creatingFirst = agents.length === 0
  const agent = createdAgent ?? eligible.find(candidate => candidate.agent_id === agentId)
  const harness = createNew
    ? availableHarnesses.find(candidate => candidate.id === harnessId)
    : harnesses.find(candidate => candidate.id === agent?.harness_id && candidate.digest === agent?.harness_digest)
  const validNewAgentId = newAgentId.trim().length > 0 && newAgentId.trim().length <= 256 && !/\p{Cc}/u.test(newAgentId)
  const ready = Boolean((agent || (createNew && validNewAgentId)) && harness?.available && input.trim())

  async function submit(event: FormEvent) {
    event.preventDefault()
    if ((!agent && !createNew) || !harness?.available || !input.trim() || (createNew && !validNewAgentId)) return
    setPending(true)
    setError(undefined)
    let definitionCreated = false
    try {
      const definition = agent ?? await createAgentFromHarness(newAgentId.trim(), harness)
      if (!agent) {
        definitionCreated = true
        setCreatedAgent(definition)
      }
      const trimmedInput = input.trim()
      const intent = [definition.agent_id, definition.version, trimmedInput].join('\u0000')
      if (pendingStart.current?.intent !== intent) {
        pendingStart.current = { intent, key: crypto.randomUUID() }
      }
      const session = await runAgent(definition.agent_id, trimmedInput, pendingStart.current.key)
      pendingStart.current = undefined
      onStarted(session)
      onOpenChange(false)
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : 'The Agent session could not be started.'
      setError(definitionCreated ? `The Agent definition was created, but its session did not start. ${message}` : message)
    } finally {
      setPending(false)
    }
  }

  return <Dialog open={open} onOpenChange={pending ? undefined : onOpenChange}>
    <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-[640px]">
      <DialogHeader>
        <DialogTitle>{eligible.length ? 'New Agent Session' : creatingFirst ? 'Start your first Agent session' : 'Create an Agent and start a session'}</DialogTitle>
        <DialogDescription>{eligible.length ? 'Run one bounded request with an active immutable Agent definition.' : creatingFirst ? 'Create an immutable definition from an approved runtime bundle, then run one bounded request.' : 'No existing definition matches an available runtime bundle. Create one from an approved bundle, then run one bounded request.'}</DialogDescription>
      </DialogHeader>
      <form className="grid gap-space-5" onSubmit={submit}>
        {eligible.length ? <div className="grid gap-space-2">
          <Label htmlFor="agent-session-definition">Agent definition</Label>
          <Select value={agentId} onValueChange={setAgentId}>
            <SelectTrigger id="agent-session-definition" className="w-full"><SelectValue placeholder="Choose an active Agent" /></SelectTrigger>
            <SelectContent>{eligible.map(candidate => <SelectItem key={candidate.agent_id} value={candidate.agent_id}>{candidate.agent_id} · revision {candidate.version}</SelectItem>)}</SelectContent>
          </Select>
        </div> : createNew ? <section className="grid gap-space-4" aria-label="New Agent definition">
          <div className="grid gap-space-2">
            <Label htmlFor="agent-definition-id">Agent ID</Label>
            <Input id="agent-definition-id" value={newAgentId} onChange={event => setNewAgentId(event.target.value)} maxLength={256} placeholder="review-agent" disabled={pending || Boolean(createdAgent)} />
            <p className="text-[11px] text-aurora-text-muted">This permanent identifier names {creatingFirst ? 'the first' : 'a new'} immutable definition in the current workspace.</p>
          </div>
          <div className="grid gap-space-2">
            <Label htmlFor="agent-harness-bundle">Approved runtime bundle</Label>
            <Select value={harnessId} onValueChange={setHarnessId} disabled={pending || Boolean(createdAgent)}>
              <SelectTrigger id="agent-harness-bundle" className="w-full"><SelectValue placeholder="Choose an available bundle" /></SelectTrigger>
              <SelectContent>{availableHarnesses.map(candidate => <SelectItem key={`${candidate.id}:${candidate.digest}`} value={candidate.id}>{candidate.id}</SelectItem>)}</SelectContent>
            </Select>
            {!availableHarnesses.length ? <p className="text-xs text-aurora-warn">No available operator-approved harness bundle is configured on this server.</p> : null}
          </div>
        </section> : <Alert variant="warn"><AlertCircle /><AlertTitle>No runnable Agent</AlertTitle><AlertDescription>No active definition matches an available approved runtime bundle, and this workspace cannot create one.</AlertDescription></Alert>}

        {harness ? <section aria-label="Execution bundle" className="grid gap-space-3">
          <div className="flex items-center gap-space-2"><Cpu className="size-4 text-aurora-accent-strong" /><h3 className="font-display text-sm font-bold text-aurora-text-primary">Operator-provisioned host runtime</h3></div>
          <dl className={META}>
            {[
              ['Harness bundle', harness.id],
              ['Repository reference', shortDigest(agent?.repository_digest ?? harness.repository_digest)],
              ['Base image reference', shortDigest(agent?.image_digest ?? harness.image_digest)],
              ['Loadout', (agent?.loadout_digest ?? harness.loadout_digest) === EMPTY_LOADOUT_DIGEST ? 'Empty loadout · [] pin' : shortDigest(agent?.loadout_digest ?? harness.loadout_digest)],
              ['Catalog generation', agent?.catalog_generation ?? harness.catalog_generation],
            ].map(([label, value]) => <div className="contents" key={label}><dt className="text-aurora-text-muted">{label}</dt><dd className="min-w-0 break-all font-mono text-aurora-text-primary" title={value}>{value}</dd></div>)}
          </dl>
          <p className="text-[11px] leading-relaxed text-aurora-text-muted">{createNew ? 'Labby will copy these exact server-approved references into the new definition before starting its configured host harness.' : 'Labby verifies these references against the selected definition before launching its configured host harness.'} They describe the approved runtime bundle; this session does not select or provision a separate container.</p>
          {harness?.available ? <Alert variant="success"><CheckCircle2 /><AlertTitle>Harness available</AlertTitle><AlertDescription>The configured executable and working directory are available to the Labby runtime.</AlertDescription></Alert> : <Alert variant="warn"><AlertCircle /><AlertTitle>Harness unavailable</AlertTitle><AlertDescription>{harness ? 'The configured executable or working directory is unavailable.' : 'No configured harness matches every pin in this Agent revision.'}</AlertDescription></Alert>}
        </section> : null}

        <div className="grid gap-space-2">
          <Label htmlFor="agent-session-input">Session input</Label>
          <Textarea id="agent-session-input" value={input} onChange={event => setInput(event.target.value)} maxLength={1024 * 1024} rows={6} placeholder="Describe the bounded work for this Agent session…" disabled={pending} />
          <p className="text-[11px] text-aurora-text-muted">The exact input and its digest are retained with the session evidence.</p>
        </div>

        {error ? <Alert variant="error"><AlertCircle /><AlertTitle>Session did not start</AlertTitle><AlertDescription>{error}</AlertDescription></Alert> : null}
        <div className="flex justify-end gap-space-2">
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={pending}>Cancel</Button>
          <Button type="submit" disabled={!ready || pending}><Play />{pending ? 'Starting…' : 'Start Session'}</Button>
        </div>
      </form>
    </DialogContent>
  </Dialog>
}
