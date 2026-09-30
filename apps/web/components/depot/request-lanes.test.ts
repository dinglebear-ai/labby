import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { RequestLanes } from './request-lanes.ts'

describe('RequestLanes', () => {
  it('keeps list and detail generations independent', () => {
    const lanes = new RequestLanes()
    const list = lanes.begin('list')
    const detail = lanes.begin('detail')
    assert.equal(lanes.isCurrent('list', list), true)
    assert.equal(lanes.isCurrent('detail', detail), true)
  })

  it('invalidates only the superseded request lane', () => {
    const lanes = new RequestLanes()
    const staleList = lanes.begin('list')
    const detail = lanes.begin('detail')
    const currentList = lanes.begin('list')
    assert.equal(lanes.isCurrent('list', staleList), false)
    assert.equal(lanes.isCurrent('list', currentList), true)
    assert.equal(lanes.isCurrent('detail', detail), true)
  })
})

it('context changes invalidate in-flight pagination, details and import preparation immediately', () => {
  const lanes = new RequestLanes()
  const pending = (['list', 'detail', 'import'] as const).map(lane => [lane, lanes.begin(lane)] as const)
  lanes.invalidateContext()
  for (const [lane, generation] of pending) assert.equal(lanes.isCurrent(lane, generation), false)
})
