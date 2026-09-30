import assert from 'node:assert/strict'
import test from 'node:test'

import { actionRequiresConfirmation, deleteAgentDescription } from './confirmation.ts'
import type { CatalogService } from '@/lib/types/command-catalog'

function catalog(destructive: boolean): CatalogService[] {
  return [{
    name: 'agents',
    description: 'Agents',
    category: 'automation',
    status: 'available',
    actions: [
      { action: 'agents.delete', description: 'Delete', destructive, params: [], returns: 'object' },
      { action: 'agents.suspend', description: 'Suspend', destructive: false, params: [], returns: 'object' },
    ],
  }]
}

test('confirmation follows the catalog destructive flag, not a hand-picked list', () => {
  assert.equal(actionRequiresConfirmation(catalog(true), 'agents', 'agents.delete'), true)
  assert.equal(actionRequiresConfirmation(catalog(false), 'agents', 'agents.delete'), false)
  assert.equal(actionRequiresConfirmation(catalog(true), 'agents', 'agents.suspend'), false)
})

test('an action the loaded catalog does not list fails closed', () => {
  assert.equal(actionRequiresConfirmation([], 'agents', 'agents.delete'), true)
  assert.equal(actionRequiresConfirmation(catalog(true), 'agents', 'agents.unknown'), true)
  assert.equal(actionRequiresConfirmation(catalog(true), 'tasks', 'agents.delete'), true)
})

test('delete copy names the agent and states the loss is permanent', () => {
  const copy = deleteAgentDescription('release-reviewer')
  assert.match(copy, /release-reviewer/)
  assert.match(copy, /cannot be restored/)
})
