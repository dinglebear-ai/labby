import assert from 'node:assert/strict'
import test from 'node:test'

import { resolveBuildId } from '../next.config.mjs'

test('uses an explicit build ID without consulting git', () => {
  let gitWasCalled = false
  const buildId = resolveBuildId({
    environment: { NODE_ENV: 'test', NEXT_BUILD_ID: 'release_484-1' },
    readGitRevision: () => {
      gitWasCalled = true
      return 'unreachable'
    },
    createFallbackId: () => 'unreachable',
  })

  assert.equal(buildId, 'release_484-1')
  assert.equal(gitWasCalled, false)
})

test('uses the git revision when no explicit build ID is provided', () => {
  const buildId = resolveBuildId({
    environment: { NODE_ENV: 'test' },
    readGitRevision: () => '0123456789abcdef0123456789abcdef01234567\n',
    createFallbackId: () => 'unreachable',
  })

  assert.equal(buildId, '0123456789abcdef0123456789abcdef01234567')
})

test('creates unique safe fallbacks when git metadata is unavailable', () => {
  const resolveArchiveBuildId = () => resolveBuildId({
    environment: { NODE_ENV: 'test' },
    readGitRevision: () => {
      throw new Error('git is unavailable')
    },
  })

  const firstBuildId = resolveArchiveBuildId()
  const secondBuildId = resolveArchiveBuildId()
  assert.match(firstBuildId, /^archive-[0-9a-f-]{36}$/)
  assert.match(secondBuildId, /^archive-[0-9a-f-]{36}$/)
  assert.notEqual(firstBuildId, secondBuildId)
})

test('rejects unsafe explicit build IDs', () => {
  assert.throws(
    () => resolveBuildId({
      environment: { NODE_ENV: 'test', NEXT_BUILD_ID: '../escape' },
      readGitRevision: () => 'unreachable',
      createFallbackId: () => 'unreachable',
    }),
    /NEXT_BUILD_ID/,
  )
})
