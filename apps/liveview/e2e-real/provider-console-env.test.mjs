import {test} from 'node:test'
import assert from 'node:assert/strict'
import {mkdtempSync, mkdirSync, rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'

import {providerConsoleEnv} from './provider-console-env.mjs'

// Losing archive selection must break these tests at the real environment boundary.
test('console retains private state and explicitly selects installed parent archives', () => {
  const root = mkdtempSync(join(tmpdir(), 'labby-mix-env-'))
  try {
    const originalHome = join(root, 'operator')
    const archives = join(originalHome, '.mix', 'archives')
    mkdirSync(archives, {recursive: true})
    const env = providerConsoleEnv({
      HOME: originalHome, PATH: '/selected/bin', LANG: 'en_US.UTF-8',
      MIX_BUILD_PATH: '/foreign/build', LABBY_E2E_PROVIDER_PASSWORD: 'secret',
      LABBY_AUTHELIA_CLIENT_SECRET: 'secret', UNRELATED: 'foreign',
    }, {home: '/private/home', temporary: '/private/tmp'})
    assert.deepEqual(env, {
      PATH: '/selected/bin', LANG: 'en_US.UTF-8', HOME: '/private/home',
      TMPDIR: '/private/tmp', MIX_ENV: 'test', MIX_ARCHIVES: archives, ERL_FLAGS: '+S 4:4',
    })
  } finally {
    rmSync(root, {recursive: true, force: true})
  }
})

test('console respects explicit installed archive and Mix home selection and scheduler bounds', () => {
  const root = mkdtempSync(join(tmpdir(), 'labby-mix-custom-'))
  try {
    const archives = join(root, 'archives')
    mkdirSync(archives)
    const state = {home: '/private/home', temporary: '/private/tmp'}
    const parent = {HOME: '/operator', MIX_HOME: root, ERL_FLAGS: '+S 2:2'}
    assert.equal(providerConsoleEnv(parent, state).MIX_ARCHIVES, archives)
    const env = providerConsoleEnv({...parent, MIX_ARCHIVES: root}, state)
    assert.equal(env.MIX_ARCHIVES, root)
    assert.equal(env.ERL_FLAGS, '+S 2:2')
    assert.equal(env.MIX_HOME, undefined)
    assert.equal(env.LANG, 'C.UTF-8')
    assert.throws(() => providerConsoleEnv({...parent, MIX_ARCHIVES: join(root, 'missing')}, state),
      /PROVIDER_MIX_ARCHIVES_UNAVAILABLE/)
    assert.throws(() => providerConsoleEnv({PATH: '/bin'}, state), /PROVIDER_MIX_ARCHIVES_UNAVAILABLE/)
  } finally {
    rmSync(root, {recursive: true, force: true})
  }
})
