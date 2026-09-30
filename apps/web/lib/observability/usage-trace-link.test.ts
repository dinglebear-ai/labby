import test from 'node:test'
import assert from 'node:assert/strict'
import { usageTraceHref } from './usage-trace-link'

test('Usage links carry only the literal upstream into the Traces query', () => {
  assert.equal(usageTraceHref('Fixture Corpus::search'), '/traces/?search=Fixture%20Corpus')
  assert.equal(usageTraceHref('server & admin=yes::tool'), '/traces/?search=server%20%26%20admin%3Dyes')
  assert.equal(usageTraceHref('single'), '/traces/?search=single')
})
