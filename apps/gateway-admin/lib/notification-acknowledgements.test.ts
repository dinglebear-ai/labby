import test from 'node:test'
import assert from 'node:assert/strict'
import { acknowledgeNotifications, reconcileNotifications, unreadNotifications, boundInactiveNotifications, notificationScope } from './notification-acknowledgements'
const warning = { key: 'gateway:alpha:warning:timeout', fingerprint: 'timeout:connection refused', gatewayName: 'alpha', message: 'Connection refused' }

test('acknowledgement survives identical polls and returns only after actual recovery or changed warning', () => {
  const initial = reconcileNotifications({}, 'warnings', [warning])
  const cleared = acknowledgeNotifications(initial)
  assert.equal(unreadNotifications(cleared).length, 0)
  assert.equal(reconcileNotifications(cleared, 'warnings', [warning]), cleared)
  const recovered = reconcileNotifications(cleared, 'warnings', [])
  const recurrence = reconcileNotifications(recovered, 'warnings', [warning])
  assert.deepEqual(unreadNotifications(recurrence).map(item => item.key), ['gateway:alpha:warning:timeout'])
  assert.equal(recurrence[warning.key].occurrence, 2)
  const changed = reconcileNotifications(cleared, 'warnings', [{ ...warning, fingerprint: 'timeout:new backend failure' }])
  assert.equal(unreadNotifications(changed).length, 1)
})

test('clear all acknowledges every producer without treating another surface as recovery', () => {
  const second = { key: 'gateway:beta:disconnected', fingerprint: 'disconnected', gatewayName: 'beta', message: 'Disconnected' }
  const combined = reconcileNotifications(reconcileNotifications({}, 'warnings', [warning]), 'runtime', [second])
  assert.equal(unreadNotifications(combined).length, 2)
  const restored = JSON.parse(JSON.stringify(acknowledgeNotifications(combined)))
  const polled = reconcileNotifications(restored, 'runtime', [second])
  assert.equal(unreadNotifications(polled).length, 0)
  assert.equal(polled[warning.key].active, true)
})


test('retention caps inactive incidents while retaining current acknowledgements', () => {
  let ledger = {}
  for (let i = 0; i < 300; i++) ledger = reconcileNotifications(ledger, 'warnings', [{ ...warning, key: `warning-${i}` }])
  const cleared = boundInactiveNotifications(acknowledgeNotifications(ledger))
  assert.equal(Object.keys(cleared).length, 257)
  assert.equal(unreadNotifications(cleared).length, 0)
  assert.equal(Object.hasOwn(cleared, 'warning-0'), false)
  assert.equal(Object.hasOwn(cleared, 'warning-299'), true)
})

test('scope is deployment and principal bound without workspace selection identity', () => {
  assert.equal(notificationScope('/v1', 'alice', 'org'), '["/v1","alice","org"]')
  assert.notEqual(notificationScope('/v1', 'alice', 'org'), notificationScope('/v1', 'bob', 'org'))
  assert.notEqual(notificationScope('/v1', 'alice', 'org'), notificationScope('https://other/v1', 'alice', 'org'))
})
