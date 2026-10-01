'use client'

import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { SUPPORTED_EXTERNAL_CLIENTS, externalClientCommand, type ExternalClient } from '@/lib/settings/external-clients'
import { SettingsCard, SettingsRow } from './SettingsChrome'

export function ExternalClientSetup({ mcpEndpoint }: { mcpEndpoint?: unknown }): React.ReactElement {
  const [clients, setClients] = useState<ExternalClient[]>([])
  const [copied, setCopied] = useState<string>()
  const localCommand = externalClientCommand(clients, 'saved-cli')
  const oauthCommand = externalClientCommand(clients, 'oauth', mcpEndpoint)

  async function copy(command: string): Promise<void> {
    try { await navigator.clipboard.writeText(command); setCopied('Command copied. Run it on the computer where these clients are installed.') }
    catch { setCopied('Could not copy. Select and copy the displayed command.') }
  }

  return <SettingsCard title="Use Labby in your applications" description="Choose applications installed on your computer. This browser cannot edit their local configuration, including when the Labby server runs on another machine.">
    {SUPPORTED_EXTERNAL_CLIENTS.map(client => <SettingsRow key={client.value} label={client.label} htmlFor={`client-${client.value}`} description={`Register Labby in ${client.label} while preserving its other MCP servers. A conflicting Labby entry needs your attention.`} control={<Checkbox id={`client-${client.value}`} checked={clients.includes(client.value)} onCheckedChange={checked => { setCopied(undefined); setClients(current => checked === true ? [...current, client.value] : current.filter(value => value !== client.value)) }} />} />)}
    <SettingsRow label="Use your saved Labby connection" description="Run this command in a terminal on your computer as your normal user. Labby verifies the protected saved gateway connection and its local stdio bridge, then registers your selected applications with backups. The credential stays in Labby's protected storage." layout="stacked">
      {localCommand ? <><code className="block break-all text-xs text-aurora-text-primary">{localCommand}</code><Button size="sm" variant="outline" onClick={() => void copy(localCommand)}>Copy local command</Button></> : <p className="text-xs text-aurora-text-muted">Select at least one application to show its setup command.</p>}
      <p className="text-xs text-aurora-text-muted">First connect this computer to Labby with the installer or CLI. If you selected a custom state directory, add --state-root followed by its absolute path. The saved connection bridge supports macOS and Linux.</p>
    </SettingsRow>
    <SettingsRow label="Connect through public OAuth" description="For a gateway with public OAuth, first sign this computer in with labby auth login using the same state directory. Connect verifies the MCP address and issues protected, application-specific observation proofs. Each application completes its own OAuth login as the same user. The control plane and MCP endpoint must share an origin in this supported flow." layout="stacked">
      {oauthCommand ? <><code className="block break-all text-xs text-aurora-text-primary">{oauthCommand}</code><Button size="sm" variant="outline" onClick={() => void copy(oauthCommand)}>Copy OAuth command</Button></> : <p className="text-xs text-aurora-text-muted">Select an application and configure an HTTPS MCP address ending in /mcp, with its control plane on the same origin, to show the OAuth command. OAuth support is checked by the local helper before it writes configuration.</p>}
      <p className="text-xs text-aurora-text-muted">After registration, use codex mcp login lab in Codex, or /mcp in Claude Code to authenticate Labby. Observation proofs expire after one hour; rerun connect to renew verification. Expiry ends observation without revoking your OAuth login.</p>
    </SettingsRow>
    <SettingsRow label="Verify use from each application" description="Restart each selected application and invoke a Labby tool. A verified local bridge confirms transport and authentication; OAuth login required means registration is waiting for login. The gateway records readiness after successful tool calls from every selected registered application using its own observation proof. Refresh the readiness checks afterward. This page does not mark applications connected." />
    {copied ? <p role="status" className="px-4 pb-3 text-xs text-aurora-text-muted">{copied}</p> : null}
  </SettingsCard>
}
