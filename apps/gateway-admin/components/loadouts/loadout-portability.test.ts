import test from 'node:test'
import assert from 'node:assert/strict'

import type { GatewayLoadout } from '../../lib/types/gateway.ts'
import { portableLoadoutFilename, portableLoadoutManifest, portableLoadoutSource } from './loadout-portability.ts'

const loadout: GatewayLoadout = {
  name: 'Research Workbench',
  description: 'Portable research stack',
  upstreams: ['zeta', 'axon'],
  services: ['setup-primary'],
  expose_code_mode: true,
  expose_tools: true,
  expose_resources: true,
  expose_prompts: true,
  expose_skills: true,
}

test('portable manifest retains granular bundle references and capability policy', () => {
  const manifest = portableLoadoutManifest(loadout, 'claude-code')
  assert.equal(manifest.kind, 'loadout')
  assert.equal(manifest.spec.target, 'claude-code')
  assert.deepEqual(manifest.spec.mcpServers, ['axon', 'zeta'])
  assert.deepEqual(manifest.spec.plugins, ['setup-primary'])
  assert.deepEqual(manifest.spec.artifacts, { tools: true, resources: true, prompts: true, skills: true, codeMode: true })
})

test('portable export is stable and uses a filesystem-safe name', () => {
  assert.equal(portableLoadoutFilename(loadout), 'research-workbench.loadout.json')
  assert.equal(portableLoadoutFilename(loadout, 'gemini-cli'), 'research-workbench.gemini-cli.loadout.json')
  assert.equal(portableLoadoutSource(loadout), `${JSON.stringify(portableLoadoutManifest(loadout), null, 2)}\n`)
})
