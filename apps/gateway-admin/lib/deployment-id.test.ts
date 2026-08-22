import assert from 'node:assert/strict'
import test from 'node:test'

import nextConfig from '../next.config.mjs'

test('static exports identify their source revision for stale-client navigation recovery', () => {
  assert.match(
    nextConfig.deploymentId ?? '',
    /^[0-9a-f]{40}$/,
    'the export must carry a git revision deployment ID',
  )
})
