import test from 'node:test'
import assert from 'node:assert/strict'

import { mockServiceActions, mockServiceConfigs, mockSupportedServices } from './mock-data.ts'

test('mockSupportedServices exposes selectable Lab services for the gateway dialog', () => {
  assert.ok(mockSupportedServices.length > 0)
  assert.deepEqual(
    mockSupportedServices.map((service) => service.key),
    ['filesystem', 'github', 'slack', 'memory'],
  )
})

test('mock service config catalog includes required fields for supported services', () => {
  assert.deepEqual(mockServiceConfigs.github.fields.map((field) => field.name), [
    'GITHUB_URL',
    'GITHUB_TOKEN',
  ])
  assert.equal(mockServiceConfigs.filesystem.configured, true)
})

test('mock service actions catalog surfaces service tools for previews', () => {
  assert.equal(mockServiceActions.memory.length, 4)
  assert.equal(mockServiceActions.slack[0]?.name, 'send_message')
})
