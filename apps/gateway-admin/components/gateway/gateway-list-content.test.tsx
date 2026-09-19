import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { GatewayListView } from './gateway-list-content'
import { GatewayHero } from './gateway-hero'
import { SidebarProvider } from '@/components/ui/sidebar'
import type { Gateway } from '@/lib/types/gateway'

const gatewayFixtures: Gateway[] = [
  {
    id: 'gw_lab',
    name: 'Lab Core',
    transport: 'stdio',
    source: 'in_process',
    configured: true,
    enabled: true,
    config: { command: 'lab service mcp --stdio --services chrome-dev-tools' },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 5,
      exposed_tool_count: 5,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: {
      tools: [{ name: 'click', description: 'Click an element', exposed: true, matched_by: 'all' }],
      resources: [],
      prompts: [],
    },
    warnings: [],
    created_at: '2026-04-17T10:00:00Z',
    updated_at: '2026-04-18T10:00:00Z',
  },
  {
    id: 'gw_http',
    name: 'Arcane',
    transport: 'http',
    source: 'custom',
    configured: true,
    enabled: true,
    config: { url: 'https://arcane.example.com/mcp' },
    status: {
      healthy: false,
      connected: false,
      discovered_tool_count: 11,
      exposed_tool_count: 7,
      discovered_resource_count: 2,
      exposed_resource_count: 1,
      discovered_prompt_count: 1,
      exposed_prompt_count: 0,
    },
    discovery: {
      tools: [{ name: 'container.restart', description: 'Restart a container', exposed: false, matched_by: null }],
      resources: [],
      prompts: [],
    },
    warnings: [
      { code: 'unreachable', message: 'Server is not responding.', timestamp: '2026-04-18T11:00:00Z' },
    ],
    created_at: '2026-04-17T10:00:00Z',
    updated_at: '2026-04-18T10:00:00Z',
  },
]

test('gateway list view renders quick-lens cards and primary actions', () => {
  const markup = renderToStaticMarkup(
    <SidebarProvider>
      <GatewayListView
        summary={{
          enabled: 3,
          healthy: 1,
          disconnected: 1,
          tools: 2,
          totalServers: 3,
          exposedTools: 2,
          discoveredPrompts: 1,
          exposedPrompts: 1,
          discoveredResources: 1,
          exposedResources: 1,
          discoveredSkills: 0,
          exposedSkills: 0,
          serverStates: [
            { id: 'a', name: 'a', color: 'var(--aurora-success)', state: 'healthy' },
            { id: 'b', name: 'b', color: 'var(--aurora-error)', state: 'disconnected' },
            { id: 'c', name: 'c', color: 'var(--aurora-accent-primary)', state: 'discovering' },
          ],
        }}
        showToolsView={false}
        gatewayFilters={{ primaryLens: 'enabled', search: '', status: [], source: [], transport: [] }}
        toolFilters={{ search: '', gatewayIds: [], exposure: 'all', source: [], transport: [] }}
        gatewayOptions={gatewayFixtures.map((gateway) => ({ value: gateway.id, label: gateway.name }))}
        activeSearch=""
        mobileSheetOpen={false}
        density="comfortable"
        isLoading={false}
        itemsCount={gatewayFixtures.length}
        filteredGateways={gatewayFixtures}
        filteredToolRows={[
          {
            gatewayId: 'gw_lab',
            gatewayName: 'Lab Core',
            source: 'in_process',
            sourceFacet: 'lab',
            transport: 'stdio',
            toolName: 'click',
            description: 'Click an element',
            exposed: true,
          },
        ]}
        onPrimaryLensChange={() => {}}
        onBackToGateways={() => {}}
        onMobileSheetOpenChange={() => {}}
        onSearchChange={() => {}}
        onGatewayFilterToggle={() => {}}
        onToolFilterToggle={() => {}}
        onExposureChange={() => {}}
        onClearFilters={() => {}}
        onCreate={() => {}}
        onEdit={() => {}}
        onTest={() => {}}
        onReload={() => {}}
        onReloadVisible={() => {}}
        isReloadingVisible={false}
        onToggleEnabled={() => {}}
        onCleanup={() => {}}
        onClearCleanupHistory={() => {}}
        onDelete={() => {}}
      />
    </SidebarProvider>,
  )

  assert.match(markup, /data-gateway-stat="healthy"/)
  assert.match(markup, /data-gateway-stat="enabled"/)
  assert.match(markup, /data-gateway-stat="disconnected"/)
  assert.match(markup, /data-gateway-stat="tools"/)
  assert.match(markup, /aria-label="Needs attention: 1"/)
  assert.match(markup, /aria-label="Gateway actions, search and filters"/)
  assert.doesNotMatch(markup, /data-gateway-filters="all-viewports"/)
  assert.match(markup, /aria-label="Reload visible servers"/)
  assert.match(markup, />3</)
})

test('gateway hero reports discovery as in progress instead of nominal health', () => {
  const markup = renderToStaticMarkup(
    <GatewayHero
      totalServers={1}
      healthy={0}
      enabled={1}
      disconnected={0}
      discoveredTools={0}
      exposedTools={0}
      discoveredPrompts={0}
      exposedPrompts={0}
      discoveredResources={0}
      exposedResources={0}
      discoveredSkills={0}
      exposedSkills={0}
      serverStates={[
        { id: 'warming', name: 'warming', color: 'var(--aurora-accent-primary)', state: 'discovering' },
      ]}
      activeLens="enabled"
      toolsViewActive={false}
      onLensChange={() => {}}
    />,
  )

  assert.match(markup, />1 discovering<\/span>/)
  assert.match(markup, /aria-label="Gateway status: 1 discovering"/)
  assert.doesNotMatch(markup, /all systems nominal/)
})

test('gateway hero does not report a disabled-only fleet as nominal', () => {
  const markup = renderToStaticMarkup(
    <GatewayHero
      totalServers={1}
      healthy={0}
      enabled={0}
      disconnected={0}
      discoveredTools={0}
      exposedTools={0}
      discoveredPrompts={0}
      exposedPrompts={0}
      discoveredResources={0}
      exposedResources={0}
      discoveredSkills={0}
      exposedSkills={0}
      serverStates={[
        { id: 'disabled', name: 'disabled', color: 'var(--aurora-text-muted)', state: 'disabled' },
      ]}
      activeLens="configured"
      toolsViewActive={false}
      onLensChange={() => {}}
    />,
  )

  assert.match(markup, />no active servers<\/span>/)
  assert.match(markup, /aria-label="Gateway status: no active servers"/)
  assert.doesNotMatch(markup, /all systems nominal/)
})
