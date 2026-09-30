import test from 'node:test'
import assert from 'node:assert/strict'
import { readableTarget } from './readable-target'

test('readable targets preserve namespace and dimension qualifiers', () => {
  assert.equal(readableTarget('github::search_repositories'), 'Search repositories · github')
  assert.equal(readableTarget('browser::getCurrentPage · resource.read'), 'Get Current Page · browser · Read resource')
  assert.equal(readableTarget('list-tools'), 'List tools')
})

test('nested resource identifiers display their terminal resource name', () => {
  assert.equal(readableTarget('qa-vm-service::lab://upstream/qa-vm-service/qa-vm-service://task-contract · resource.read'), 'Task contract · qa-vm-service · Read resource')
  assert.equal(readableTarget('claude-macpoo::TaskOutput'), 'Task Output · claude-macpoo')
})
