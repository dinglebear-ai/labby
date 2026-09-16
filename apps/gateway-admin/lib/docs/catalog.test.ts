import assert from 'node:assert/strict'
import test from 'node:test'

import {
  encodeDocAssetPath,
  parseDocsManifest,
  resolveDocHref,
} from './catalog'

test('parses the generated docs manifest contract', () => {
  assert.deepEqual(
    parseDocsManifest({
      version: 1,
      documents: [
        { path: 'README.md', title: 'Labby', section: 'Overview', bytes: 42, status: null },
      ],
    }),
    {
      version: 1,
      documents: [
        { path: 'README.md', title: 'Labby', section: 'Overview', bytes: 42, status: null },
      ],
    },
  )
  assert.deepEqual(
    parseDocsManifest({
      version: 1,
      documents: [{ path: 'legacy.md', title: 'Legacy', section: 'Test', bytes: 1 }],
    }),
    {
      version: 1,
      documents: [{ path: 'legacy.md', title: 'Legacy', section: 'Test', bytes: 1, status: null }],
    },
  )
  assert.equal(parseDocsManifest({ version: 2, documents: [] }), null)
  assert.equal(
    parseDocsManifest({
      version: 1,
      documents: [{ path: '../SECRET.md', title: 'Secret', section: 'Bad', bytes: 1, status: null }],
    }),
    null,
  )
  assert.equal(
    parseDocsManifest({
      version: 1,
      documents: [
        { path: 'same.md', title: 'One', section: 'Test', bytes: 1, status: null },
        { path: 'same.md', title: 'Two', section: 'Test', bytes: 2, status: null },
      ],
    }),
    null,
  )
  assert.equal(
    parseDocsManifest({
      version: 1,
      documents: [{ path: 'bad.md', title: 'Bad', section: 'Test', bytes: -1, status: null }],
    }),
    null,
  )
  assert.equal(
    parseDocsManifest({
      version: 1,
      documents: [{ path: 'bad.md', title: 'Bad', section: 'Test', bytes: 1, status: 'bad\nstatus' }],
    }),
    null,
  )
})

test('rewrites relative markdown links back into the docs viewer', () => {
  const knownPaths = new Set(['services/GATEWAY.md', 'runtime/CONFIG.md'])
  assert.equal(
    resolveDocHref('services/GATEWAY.md', '../runtime/CONFIG.md#environment', knownPaths),
    '/docs?doc=runtime%2FCONFIG.md#environment',
  )
  assert.equal(
    resolveDocHref('services/GATEWAY.md', 'https://example.com/docs.md', knownPaths),
    'https://example.com/docs.md',
  )
  assert.equal(resolveDocHref('services/GATEWAY.md', '#local', knownPaths), '#local')
})

test('rewrites repo-relative non-embedded targets to canonical GitHub source URLs', () => {
  const knownPaths = new Set(['runtime/CONFIG.md', 'generated/action-catalog.md'])

  assert.equal(
    resolveDocHref('runtime/CONFIG.md', '../../config/config.example.toml', knownPaths),
    'https://github.com/dinglebear-ai/labby/blob/main/config/config.example.toml',
  )
  assert.equal(
    resolveDocHref('services/GATEWAY.md', '../generated/action-catalog.json?raw=1#actions', knownPaths),
    'https://github.com/dinglebear-ai/labby/blob/main/docs/generated/action-catalog.json?raw=1#actions',
  )
  assert.equal(
    resolveDocHref('runtime/CONFIG.md', '../archive/retired-labby/', knownPaths),
    'https://github.com/dinglebear-ai/labby/tree/main/docs/archive/retired-labby',
  )
  assert.equal(
    resolveDocHref(
      'plans/verification-toolkit/INVENTORY.md',
      '../../../crates/labby/tests/live%5Fmcp%5Factions.rs',
      knownPaths,
    ),
    'https://github.com/dinglebear-ai/labby/blob/main/crates/labby/tests/live_mcp_actions.rs',
  )
})

test('keeps repo-relative links from escaping the repository root', () => {
  const knownPaths = new Set(['README.md'])
  assert.equal(resolveDocHref('README.md', '../../outside.md', knownPaths), '../../outside.md')
  assert.equal(resolveDocHref('README.md', '%2e%2e/%2e%2e/secret.txt', knownPaths), '%2e%2e/%2e%2e/secret.txt')
  assert.equal(resolveDocHref('README.md', '/absolute.md', knownPaths), '/absolute.md')
})

test('encodes individual path segments for static asset fetches', () => {
  assert.equal(encodeDocAssetPath('guides/My Guide.md'), 'guides/My%20Guide.md')
})
