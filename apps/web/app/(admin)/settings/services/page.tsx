import Link from 'next/link'
import { ArrowUpRight, Search, Server } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { SettingsCard, SettingsRow } from '@/components/settings/SettingsChrome'

export default function McpServerSettingsPage(): React.ReactElement {
  return <SettingsCard title="MCP servers" description="Connect the tools your Agents and applications will use. Server addresses, installation inputs, permissions, and credentials are configured in the server connection flow.">
    <SettingsRow layout="stacked" label="Find a server in Discover" description="Search your configured catalog for a compatible MCP server. Add MCP server guides you through missing inputs and authorization, then checks the connection. Add to Library saves an artifact without starting a server.">
      <Button asChild data-visible-label variant="outline" className="h-9 w-fit gap-2"><Link href="/depot/"><Search aria-hidden="true" className="size-4" />Open Discover<ArrowUpRight aria-hidden="true" className="size-4" /></Link></Button>
    </SettingsRow>
    <SettingsRow layout="stacked" label="Add or manage a server connection" description="Use server management when you already have an MCP command or endpoint. Supply its required inputs and credentials, review access, and test the connection. Existing server connections can be managed here too.">
      <Button asChild data-visible-label variant="outline" className="h-9 w-fit gap-2"><Link href="/gateways/"><Server aria-hidden="true" className="size-4" />Manage MCP servers<ArrowUpRight aria-hidden="true" className="size-4" /></Link></Button>
    </SettingsRow>
    <SettingsRow label="Verify a working tool" description="A saved connection does not prove a tool works. Check the server’s capabilities and complete an approved tool call. First-use readiness records successful verification; your selected applications must also complete their own tool calls." />
  </SettingsCard>
}
