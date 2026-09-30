import test from 'node:test'
import assert from 'node:assert/strict'

import type { GatewayLoadout } from '../../lib/types/gateway.ts'
import { filterLoadouts } from './loadouts-page-content.tsx'

const loadouts: GatewayLoadout[] = [
  {
    name: 'research-workbench',
    description: 'Documentation and catalog research',
    upstreams: ['context7', 'axon'],
    services: [],
    expose_code_mode: true,
    expose_tools: true,
    expose_resources: true,
    expose_prompts: true,
    expose_skills: true,
  },
  {
    name: 'operator-console',
    description: 'Infrastructure operations',
    upstreams: ['dozzle'],
    services: ['setup-primary'],
    expose_code_mode: true,
    expose_tools: true,
    expose_resources: false,
    expose_prompts: false,
    expose_skills: false,
  },
]

test('filterLoadouts searches names, descriptions, upstreams, and services', () => {
  assert.deepEqual(filterLoadouts(loadouts, 'RESEARCH'), [loadouts[0]])
  assert.deepEqual(filterLoadouts(loadouts, 'catalog'), [loadouts[0]])
  assert.deepEqual(filterLoadouts(loadouts, 'dozzle'), [loadouts[1]])
  assert.deepEqual(filterLoadouts(loadouts, 'setup-primary'), [loadouts[1]])
})

test('filterLoadouts returns all Loadouts for a blank query', () => {
  assert.equal(filterLoadouts(loadouts, '   '), loadouts)
})
