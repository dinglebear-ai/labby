import assert from 'node:assert/strict'
import test from 'node:test'

import { gatewayDisplayName } from './gateway-display-name'

test('gateway display names clean up configured identifier delimiters and common initialisms', () => {
  assert.equal(gatewayDisplayName('agent-os_windows-mcp'), 'Agent OS Windows MCP')
  assert.equal(gatewayDisplayName('claude-in-mobile'), 'Claude in Mobile')
  assert.equal(gatewayDisplayName('nextjs-devtools'), 'Nextjs Devtools')
})

test('gateway display names preserve names that are already presentation-ready', () => {
  assert.equal(gatewayDisplayName('Asana'), 'Asana')
  assert.equal(gatewayDisplayName('Gateway beta Control Plane'), 'Gateway beta Control Plane')
  assert.equal(gatewayDisplayName('YTDL'), 'YTDL')
})
