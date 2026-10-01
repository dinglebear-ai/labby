import assert from 'node:assert/strict'
import test from 'node:test'
import type { FederatedArtifact } from '@/lib/api/depot-client'
import type { Gateway } from '@/lib/types/gateway'
import { activateCatalogMcp, catalogGatewayInput, supportedMcpConnection } from './mcp-activation'
const artifact: FederatedArtifact = { providerId: 'public', artifactId: 'a', kind: 'mcp-server', currentRevisionId: 'r1', mcpConnection: { schemaVersion: 'labby.mcp-connection/v1', revisionId: 'r1', transport: 'http', authentication: 'none', url: 'https://example.org/mcp' } }
test('activation requires exact revision and clean HTTPS metadata', () => {
  assert.ok(supportedMcpConnection(artifact))
  assert.equal(supportedMcpConnection({ ...artifact, currentRevisionId: 'r2' }), undefined)
  for (const url of ['http://example.org/mcp', 'https://user:secret@example.org/mcp', 'https://example.org/mcp?token=secret', 'https://example.org/mcp#x', 'https://example.org/\tmcp', 'https://example.org\\mcp', 'https://example.org/mcp\n']) assert.equal(supportedMcpConnection({ ...artifact, mcpConnection: { ...artifact.mcpConnection!, url } }), undefined)
})
test('activation validates name and token and limits initial capabilities', () => {
  assert.equal(catalogGatewayInput(artifact, 'server', '').config.proxy_resources, false)
  assert.throws(() => catalogGatewayInput(artifact, 'bad name', ''))
  const secured = { ...artifact, mcpConnection: { ...artifact.mcpConnection!, authentication: 'bearer' as const } }
  assert.throws(() => catalogGatewayInput(secured, 'server', ''))
  assert.throws(() => catalogGatewayInput(secured, 'server', 'value\nheader'))
  assert.equal(catalogGatewayInput(secured, 'server', 'token').config.bearer_token_value, 'token')
})
test('failed probe preserves saved server identity for retry', async () => {
  const events: string[] = []
  const result = await activateCatalogMcp(catalogGatewayInput(artifact, 'server', ''), gateway => events.push(`saved:${gateway.id}`), {
    async create() { events.push('create'); return { id: 'server' } as Gateway },
    async test(id) { events.push(`test:${id}`); return { success: false, message: 'unreachable' } },
  })
  assert.deepEqual(events, ['create', 'saved:server', 'test:server'])
  assert.equal(result.success, false)
})
