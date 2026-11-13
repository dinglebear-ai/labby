import {test} from 'node:test'
import assert from 'node:assert/strict'
import {newInvitationToken} from '../../assets/js/access_secrets.mjs'

test('a 32 byte browser random source produces a nonzero 64 hex token', () => {
  let calls = 0
  const value = newInvitationToken({getRandomValues(bytes) {
    assert.equal(bytes.length, 32)
    calls++
    bytes.fill(171)
  }})
  assert.equal(calls, 1)
  assert.equal(value, 'ab'.repeat(32))
})

test('crypto failure and an all zero source fail without a token fallback', () => {
  assert.throws(() => newInvitationToken({getRandomValues() { throw new Error('unavailable') }}))
  assert.throws(() => newInvitationToken({getRandomValues(bytes) { bytes.fill(0) }}))
})

test('disconnected native forms cannot queue a write and fresh help reopens native submission', async () => {
  const {default: hook} = await import('../../assets/js/access_secrets.mjs')
  const events = {}
  const instance = {el: {addEventListener() {}, querySelector() {return null}, querySelectorAll() {return []}},
    handleEvent(name, fn) {events[name] = fn}, pushEvent() {throw new Error('Unexpected dispatch')}}
  hook.mounted.call(instance)
  let prevented = 0
  const form = {querySelector() {return null}, reportValidity() {return true}}
  const event = {target: {closest() {return form}}, preventDefault() {prevented++}, stopImmediatePropagation() {}}
  instance.submit(event)
  assert.equal(prevented, 1)
  events['access-ready']({})
  instance.submit(event)
  assert.equal(prevented, 1)
  hook.disconnected.call(instance)
  instance.submit(event)
  assert.equal(prevented, 2)
})
