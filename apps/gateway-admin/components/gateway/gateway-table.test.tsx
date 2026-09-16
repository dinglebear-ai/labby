import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { GatewayTable } from './gateway-table'
import type { Gateway } from '@/lib/types/gateway'

const gateway: Gateway = {
  id: 'gw_1',
  name: 'Gateway beta Control Plane',
  transport: 'http',
  source: 'custom',
  configured: true,
  enabled: true,
  config: {
    url: 'https://gateway_beta.example.com/mcp',
  },
  status: {
    healthy: true,
    connected: true,
    last_error: 'Reload required to apply policy changes.',
    discovered_tool_count: 18,
    exposed_tool_count: 14,
    discovered_resource_count: 6,
    exposed_resource_count: 4,
    discovered_prompt_count: 3,
    exposed_prompt_count: 2,
  },
  discovery: {
    tools: [],
    resources: [],
    prompts: [],
  },
  warnings: [
    {
      code: 'policy_drift',
      message: 'Tool exposure differs from the last successful sync.',
      timestamp: '2026-04-17T13:00:00Z',
    },
  ],
  created_at: '2026-04-16T12:00:00Z',
  updated_at: '2026-04-17T12:00:00Z',
}

test('gateway table uses aurora lifted surfaces and muted operational pills', () => {
  const markup = renderToStaticMarkup(
    React.createElement(GatewayTable, {
      gateways: [gateway],
      density: 'comfortable',
      onEdit: () => {},
      onTest: () => {},
      onReload: () => {},
      onCleanup: () => {},
      onClearCleanupHistory: () => {},
      onToggleEnabled: () => {},
      onDelete: () => {},
    }),
  )

  assert.match(markup, /bg-aurora-panel-strong/)
  assert.match(markup, /text-aurora-text-primary/)
  assert.match(markup, /data-mobile-metric="tools"/)
  assert.match(markup, /data-mobile-metric="resources"/)
  assert.match(markup, /data-mobile-metric="prompts"/)
  assert.match(markup, /data-mobile-metric="runtime"/)
  assert.match(markup, />14</)
  assert.match(markup, /prompts/)
  assert.doesNotMatch(markup, /Reload required to apply policy changes/)
})

test('gateway table sorts servers by name and shows full stdio command line', () => {
  const stdioGateway: Gateway = {
    ...gateway,
    id: 'gw_2',
    name: 'Neo4j Memory',
    transport: 'stdio',
    config: {
      command: 'uvx',
      args: ['neo4j-memory-mcp'],
    },
    status: {
      ...gateway.status,
      discovered_tool_count: 3,
      exposed_tool_count: 3,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    warnings: [],
  }

  const zedGateway: Gateway = {
    ...gateway,
    id: 'gw_3',
    name: 'Zed Search',
    config: {
      url: 'https://zed.example.com/mcp',
    },
    warnings: [],
  }

  const markup = renderToStaticMarkup(
    React.createElement(GatewayTable, {
      gateways: [zedGateway, stdioGateway],
      density: 'comfortable',
      onEdit: () => {},
      onTest: () => {},
      onReload: () => {},
      onCleanup: () => {},
      onClearCleanupHistory: () => {},
      onToggleEnabled: () => {},
      onDelete: () => {},
    }),
  )

  assert.ok(markup.indexOf('Neo4j Memory') < markup.indexOf('Zed Search'))
  assert.match(markup, /uvx neo4j-memory-mcp/)
  assert.match(markup, /Sort by server/)
  assert.match(markup, /Sort by clients[\s\S]*Sort by exposed[\s\S]*Sort by endpoint[\s\S]*Sort by uptime/)
  assert.match(markup, /Sort by uptime/)
  assert.match(markup, /aria-sort="none"[^>]*><span>Uptime<\/span>/)
  assert.doesNotMatch(markup, /data-gateway-column="clients"/)
  assert.match(markup, /data-gateway-column="exposed"/)
  assert.match(markup, /data-gateway-column="endpoint"/)
  assert.match(markup, /data-gateway-column="uptime"/)
  assert.match(markup, /Client count is not reported by the gateway API/)
  assert.match(markup, /max-w-full justify-self-center px-2\.5 text-center/)
  assert.match(markup, /Reorder exposed column/)
  assert.match(markup, /Reorder endpoint column/)
  assert.match(markup, /Reorder runtime age column/)
})

test('gateway table presents a readable label while preserving the configured identifier', () => {
  const configured = {
    ...gateway,
    id: 'agent-os_windows-mcp',
    name: 'agent-os_windows-mcp',
  }
  const markup = renderToStaticMarkup(
    <GatewayTable gateways={[configured]} density="comfortable" onEdit={() => {}} onTest={() => {}} onReload={() => {}} onCleanup={() => {}} onClearCleanupHistory={() => {}} onToggleEnabled={() => {}} onDelete={() => {}} />,
  )

  assert.match(markup, />Agent OS Windows MCP<\/a>/)
  assert.match(markup, /href="\/gateway\?id=agent-os_windows-mcp"/)
  assert.match(markup, /title="agent-os_windows-mcp · Healthy"/)
})


test('gateway table exposes stale service removal for unknown in-process services', () => {
  const staleService: Gateway = {
    ...gateway,
    id: 'stale-registry',
    name: 'missing-service',
    transport: 'in_process',
    source: 'in_process',
    status: {
      ...gateway.status,
      healthy: false,
      connected: false,
      discovered_tool_count: 0,
      exposed_tool_count: 0,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    warnings: [
      {
        code: 'unknown_service',
        message: 'service `missing-service` is not registered in this lab binary',
        timestamp: '2026-04-25T12:00:00Z',
      },
    ],
  }

  const markup = renderToStaticMarkup(
    React.createElement(GatewayTable, {
      gateways: [staleService],
      density: 'comfortable',
      onEdit: () => {},
      onTest: () => {},
      onReload: () => {},
      onCleanup: () => {},
      onClearCleanupHistory: () => {},
      onToggleEnabled: () => {},
      onDelete: () => {},
    }),
  )

  assert.match(markup, /Remove stale service/)
  assert.doesNotMatch(markup, /Remove gateway/)
})

test('disabled servers have a separate group and never claim a connected or disconnected status', () => {
  const markup = renderToStaticMarkup(<GatewayTable gateways={[{ ...gateway, enabled: false, name: 'disabled-upstream' }]} density="comfortable" onEdit={() => {}} onTest={() => {}} onReload={() => {}} onCleanup={() => {}} onClearCleanupHistory={() => {}} onToggleEnabled={() => {}} onDelete={() => {}} />)
  assert.match(markup, /title="Disabled"/)
  assert.doesNotMatch(markup, />Healthy<\/span>/)
  assert.doesNotMatch(markup, /title="Disconnected"/)
  assert.match(markup, /Sort by clients/)
  assert.doesNotMatch(markup, /Reorder clients column/)
})
