import test from 'node:test'
import assert from 'node:assert/strict'
import {providerLogin} from './oauth_login.mjs'

test('bounded requests expose only opaque flow ID and destroy aborts', async () => {
  const requests = []
  const timers = []
  const view = {showLink(url) { this.url = url }, pending() {}, error() {}, complete() { this.done = true }}
  const client = providerLogin({
    request: async (path, body, signal) => {
      requests.push({path, body, signal})
      return path.endsWith('start') ? {flow_id: 'opaque', authorization_url: 'https://gateway.test/auth/desktop/authorize?state=one', expires_at: Date.now() / 1000 + 300} : {status: 'complete'}
    }, view, schedule: fn => { timers.push(fn); return timers.length }, clear: () => {}, now: Date.now,
  })
  await client.start()
  assert.equal(view.url, 'https://gateway.test/auth/desktop/authorize?state=one')
  await timers.shift()()
  assert.equal(view.done, true)
  assert.deepEqual(requests[1].body, {flow_id: 'opaque'})
  client.destroy()
  assert.equal(requests[0].signal.aborted, true)
})

test('cancel aborts work and prevents late link publication', async () => {
  let resolve
  const view = {showLink() { assert.fail('late link') }, pending() {}, error() {}, complete() {}}
  const client = providerLogin({request: () => new Promise(r => {resolve = r}), view, schedule: () => 1, clear: () => {}, now: Date.now})
  const pending = client.start()
  client.destroy()
  resolve({flow_id: 'opaque', authorization_url: 'https://gateway.test', expires_at: Date.now() / 1000 + 300})
  await pending
})

test('lost successful poll response recovers once through the same flow', async () => {
  const calls = []
  const timers = []
  let polls = 0
  const view = {showLink() {}, pending() {}, error() { assert.fail('recoverable delivery loss') }, complete() { this.done = true }}
  const client = providerLogin({request: async (path, body) => {
    calls.push({path, body})
    if (path.endsWith('start')) return {flow_id: 'same-flow', authorization_url: 'https://gateway.test/auth/desktop/authorize?state=one', expires_at: Date.now() / 1000 + 300}
    if (++polls === 1) throw new Error('response_lost')
    return {status: 'complete'}
  }, view, schedule: fn => { timers.push(fn); return timers.length }, clear: () => {}, now: Date.now})
  await client.start()
  await timers.shift()()
  assert.equal(timers.length, 1)
  await timers.shift()()
  assert.equal(view.done, true)
  assert.deepEqual(calls.slice(1).map(call => call.body), [{flow_id: 'same-flow'}, {flow_id: 'same-flow'}])
  client.destroy()
})

test('existing signed flow survives a rejected restart for explicit reset', async () => {
  const calls = []
  const timers = []
  const view = {showLink() {}, pending() {}, error() { this.failed = true }, reset() { this.cleared = true }, complete() { assert.fail('unexpected completion') }}
  const client = providerLogin({initialFlow: 'retained-flow', request: async (path, body) => {
    calls.push({path, body})
    if (path.endsWith('start')) throw new Error('sign_out_required')
    return {status: 'cancelled'}
  }, view, schedule: fn => { timers.push(fn); return timers.length }, clear: () => {}, now: Date.now})
  await client.start()
  assert.equal(view.failed, true)
  await client.cancel()
  assert.deepEqual(calls.at(-1).body, {flow_id: 'retained-flow'})
  assert.equal(view.cleared, true)
  client.destroy()
})

test('two lost poll deliveries stop at one recovery and allow explicit reset', async () => {
  const timers = []
  const calls = []
  const view = {showLink() {}, pending() {}, error() { this.failed = true }, reset() { this.cleared = true }, complete() { assert.fail('unexpected completion') }}
  const client = providerLogin({request: async (path, body) => {
    calls.push({path, body})
    if (path.endsWith('start')) return {flow_id: 'retained', authorization_url: 'https://gateway.test', expires_at: Date.now() / 1000 + 300}
    if (path.endsWith('cancel')) return {status: 'cancelled'}
    throw new Error('response_lost')
  }, view, schedule: fn => { timers.push(fn); return timers.length }, clear: () => {}, now: Date.now})
  await client.start()
  await timers.shift()()
  await timers.shift()()
  assert.equal(timers.length, 0)
  assert.equal(view.failed, true)
  assert.equal(calls.filter(call => call.path.endsWith('poll')).length, 2)
  await client.cancel()
  assert.deepEqual(calls.at(-1).body, {flow_id: 'retained'})
  assert.equal(view.cleared, true)
  client.destroy()
})

test('destruction aborts reset and suppresses its late UI result', async () => {
  let finish
  let signal
  const client = providerLogin({initialFlow: 'retained', request: (_path, _body, active) => {
    signal = active
    return new Promise(resolve => { finish = resolve })
  }, view: {error() { assert.fail('late error') }, reset() { assert.fail('late reset') }}, schedule: () => 1, clear: () => {}})
  const reset = client.cancel()
  client.destroy()
  assert.equal(signal.aborted, true)
  finish({status: 'cancelled'})
  await reset
})
