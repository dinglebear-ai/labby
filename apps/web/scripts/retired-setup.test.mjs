import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import test from 'node:test'

const appRoot = new URL('../', import.meta.url)

for (const path of [
  'app/setup',
  'components/setup/public-setup-page.tsx',
  'public/setup',
]) {
  test(`retired web setup is not shipped: ${path}`, () => {
    assert.equal(existsSync(new URL(path, appRoot)), false)
  })
}

test('console components have no legacy public setup mode', () => {
  for (const path of [
    'components/console/console-shell.tsx',
    'components/console/console-sidebar.tsx',
    'components/console/console-topbar.tsx',
  ]) {
    assert.doesNotMatch(readFileSync(new URL(path, appRoot), 'utf8'), /publicSetup/)
  }
})

test('current settings and owner onboarding retain their shared implementation', () => {
  for (const path of [
    'app/(admin)/settings/page.tsx',
    'components/auth/owner-setup-screen.tsx',
    'components/setup/ServiceForm.tsx',
    'lib/api/setup-client.ts',
  ]) {
    assert.equal(existsSync(new URL(path, appRoot)), true, path)
  }
})
