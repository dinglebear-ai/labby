'use client'

import { useEffect, useState } from 'react'

import { CheckCircle2, ChevronRight, Globe2, Loader2, Settings2, TerminalSquare } from 'lucide-react'

import { Input } from '@/components/ui/input'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import type { TransportType } from '@/lib/types/gateway'
import { cn } from '@/lib/utils'
import { gatewayAction } from '@/lib/api/gateway-client'
import { selectedSshStdioHost, sshStdioCommand } from '@/lib/ssh-stdio-command'

const inputClassName =
  'border-aurora-border-strong bg-aurora-page-bg/80 shadow-[var(--aurora-highlight-medium)] placeholder:text-aurora-text-muted/70 hover:border-aurora-accent-primary/35 focus-visible:bg-aurora-control-surface'

interface GatewayCustomConnectionFormProps {
  transport: TransportType
  onTransportChange: (transport: TransportType) => void
  name: string
  onNameChange: (name: string) => void
  displayName: string
  onDisplayNameChange: (displayName: string) => void
  url: string
  onUrlChange: (url: string) => void
  command: string
  onCommandChange: (command: string) => void
  envText: string
  onEnvTextChange: (text: string) => void
  envCount: number
  errors: Record<string, string>
  isProbing: boolean
  oauthDiscovered: boolean
}

export function GatewayCustomConnectionForm(props: GatewayCustomConnectionFormProps) {
  const {
    transport, onTransportChange, name, onNameChange, displayName, onDisplayNameChange, url, onUrlChange,
    command, onCommandChange, envText, onEnvTextChange, envCount, errors,
    isProbing, oauthDiscovered,
  } = props
  const [sshHosts, setSshHosts] = useState<string[]>([])
  const [sshHost, setSshHost] = useState('local')
  const [manualCommand, setManualCommand] = useState('')
  const [remoteCommand, setRemoteCommand] = useState('')
  const [sshHostsError, setSshHostsError] = useState<string | null>(null)

  useEffect(() => {
    if (transport !== 'stdio') return
    const controller = new AbortController()
    gatewayAction<string[]>('gateway.ssh_hosts.list', {}, controller.signal)
      .then((hosts) => {
        setSshHosts(hosts)
        setSshHostsError(null)
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return
        setSshHostsError(error instanceof Error ? error.message : 'SSH hosts could not be loaded')
      })
    return () => controller.abort()
  }, [transport])

  useEffect(() => {
    if (sshHost !== 'local') return
    const selected = selectedSshStdioHost(command)
    if (!selected || !sshHosts.includes(selected.host)) return
    setSshHost(selected.host)
    setRemoteCommand(selected.remoteCommand)
  }, [command, sshHost, sshHosts])

  function selectSshHost(host: string) {
    if (sshHost === 'local' && host !== 'local') setManualCommand(command)
    setSshHost(host)
    if (host === 'local') {
      onCommandChange(manualCommand)
    } else {
      onCommandChange(sshStdioCommand(host, remoteCommand))
    }
  }

  function updateRemoteCommand(value: string) {
    setRemoteCommand(value)
    onCommandChange(sshStdioCommand(sshHost, value))
  }

  return (
    <div className="order-1 rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface/70 p-4 shadow-[var(--aurora-highlight-medium)]">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-1">
          <p className="flex items-center gap-2 text-[13px] font-semibold text-aurora-text-primary">
            {transport === 'http' ? <Globe2 className="size-4 text-aurora-accent-primary" /> : <TerminalSquare className="size-4 text-aurora-accent-primary" />}
            Connection
          </p>
          <p className="text-[12px] leading-5 text-aurora-text-muted">
            {transport === 'http' ? 'Remote MCP endpoint.' : 'Local process launched over stdin/stdout.'}
          </p>
        </div>
        <RadioGroup
          value={transport}
          onValueChange={(value) => onTransportChange(value as TransportType)}
          className="grid grid-cols-2 overflow-hidden rounded-aurora-1 border border-aurora-border-default bg-aurora-panel-medium p-1"
        >
          {(['http', 'stdio'] as const).map((value) => (
            <label
              key={value}
              className={cn(
                'flex h-8 cursor-pointer items-center justify-center gap-2 rounded-aurora-1 px-3 text-[12px] font-semibold transition-[background-color,color,box-shadow]',
                transport === value
                  ? 'bg-aurora-accent-primary/12 text-aurora-text-primary shadow-aurora-active-glow'
                  : 'text-aurora-text-muted hover:text-aurora-text-primary',
              )}
              htmlFor={`transport-${value}`}
            >
              <RadioGroupItem value={value} id={`transport-${value}`} className="sr-only" />
              {value === 'http' ? <Globe2 className="size-3.5" /> : <TerminalSquare className="size-3.5" />}
              {value === 'http' ? 'HTTP' : 'stdio'}
            </label>
          ))}
        </RadioGroup>
      </div>

      <div className="mt-4 grid gap-4">
        <Field>
          <FieldLabel htmlFor="display_name">Display name</FieldLabel>
          <Input id="display_name" value={displayName} onChange={(event) => onDisplayNameChange(event.target.value)} placeholder="My Gateway" maxLength={80} className={cn(inputClassName, errors.display_name && 'border-destructive')} />
          {errors.display_name ? <p className="text-sm text-destructive">{errors.display_name}</p> : <FieldDescription>Any label you like. Shown in Labby only.</FieldDescription>}
        </Field>

        <Field>
          <FieldLabel htmlFor="name">ID</FieldLabel>
          <Input id="name" value={name} onChange={(event) => onNameChange(event.target.value)} placeholder="my-gateway" className={cn(inputClassName, errors.name && 'border-destructive')} />
          {errors.name ? <p className="text-sm text-destructive">{errors.name}</p> : <FieldDescription>Stable identifier used in tool names and routes. Lowercase letters, digits, hyphens. Filled from the display name or host.</FieldDescription>}
        </Field>

        {transport === 'http' ? (
          <Field>
            <FieldLabel htmlFor="url">URL</FieldLabel>
            <div className="relative">
              <Input id="url" value={url} onChange={(event) => onUrlChange(event.target.value)} placeholder="https://example.com/mcp" className={cn(inputClassName, 'pr-8', errors.url && 'border-destructive')} />
              {isProbing && <Loader2 className="pointer-events-none absolute right-2.5 top-1/2 size-4 -translate-y-1/2 animate-spin text-aurora-text-muted" />}
              {!isProbing && oauthDiscovered && <CheckCircle2 className="pointer-events-none absolute right-2.5 top-1/2 size-4 -translate-y-1/2 text-aurora-success" />}
            </div>
            {errors.url ? <p className="text-sm text-destructive">{errors.url}</p> : <FieldDescription>Labby probes this endpoint and detects OAuth support automatically.</FieldDescription>}
          </Field>
        ) : (
          <div className="space-y-4">
            <Field>
              <FieldLabel htmlFor="stdio-host">Run on</FieldLabel>
              <select
                id="stdio-host"
                value={sshHost}
                onChange={(event) => selectSshHost(event.target.value)}
                className={cn('h-9 w-full rounded-aurora-1 border px-3 text-sm text-aurora-text-primary', inputClassName)}
              >
                <option value="local">Custom command on Labby host</option>
                {sshHosts.map((host) => <option key={host} value={host}>{host} (SSH)</option>)}
              </select>
              {sshHostsError ? <p className="text-sm text-destructive">{sshHostsError}</p> : <FieldDescription>SSH devices come from the Labby gateway account&apos;s SSH config.</FieldDescription>}
            </Field>
            {sshHost !== 'local' && (
              <Field>
                <FieldLabel htmlFor="remote-command">Command on {sshHost}</FieldLabel>
                <Input
                  id="remote-command"
                  value={remoteCommand}
                  onChange={(event) => updateRemoteCommand(event.target.value)}
                  placeholder="/usr/local/bin/remote-mcp mcp"
                  className={inputClassName}
                />
                <FieldDescription>The command must start an MCP server over standard input and output on that device.</FieldDescription>
              </Field>
            )}
            <Field>
              <FieldLabel htmlFor="command">Command line</FieldLabel>
              <Input id="command" value={command} readOnly={sshHost !== 'local'} onChange={(event) => onCommandChange(event.target.value)} placeholder="npx -y @modelcontextprotocol/server-filesystem /path" className={cn(inputClassName, errors.command && 'border-destructive')} />
              {errors.command ? <p className="text-sm text-destructive">{errors.command}</p> : <FieldDescription>{sshHost === 'local' ? 'Enter the full launch command. Quoted arguments with spaces are preserved.' : 'Generated SSH launch command. Choose Custom command to edit it directly.'}</FieldDescription>}
            </Field>
          </div>
        )}

        <details className="group rounded-aurora-1 border border-aurora-border-default bg-aurora-panel-medium/50 p-3">
          <summary className="flex cursor-pointer select-none list-none items-center justify-between gap-3 text-sm font-semibold text-aurora-text-primary [&::-webkit-details-marker]:hidden">
            <span className="flex min-w-0 items-center gap-2"><ChevronRight className="size-4 shrink-0 transition-transform group-open:rotate-90" /><Settings2 className="size-4 shrink-0 text-aurora-accent-primary" />Environment</span>
            <span className="text-[12px] font-medium text-aurora-text-muted">{envCount ? `${envCount} vars` : 'Optional'}</span>
          </summary>
          <div className="mt-3 space-y-2">
            <textarea className={cn('min-h-[112px] w-full resize-none rounded-aurora-1 px-3 py-2 font-mono text-xs text-aurora-text-primary outline-none focus:border-aurora-accent-primary focus:ring-2 focus:ring-aurora-accent-primary/34', inputClassName)} placeholder={'GOOGLE_APPLICATION_CREDENTIALS=/path/to/creds.json\nMCP_LOG_LEVEL=info'} value={envText} onChange={(event) => onEnvTextChange(event.target.value)} />
            <p className="text-[12px] leading-5 text-aurora-text-muted">
              {sshHost === 'local'
                ? <>One <code>KEY=VALUE</code> per line. Saved with this server config.</>
                : 'These variables apply to SSH on the Labby host. Configure the remote server environment on the selected device.'}
            </p>
          </div>
        </details>
      </div>
    </div>
  )
}
