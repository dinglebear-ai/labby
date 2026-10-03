import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { artifactInstallCommand } from './install-command'

test('install references remain one literal shell argument', () => {
  const name = "sample; printf injected $(printf expansion) `printf expansion` ' quote"
  const command = artifactInstallCommand({ providerId: 'catalog', artifactId: 'id', namespace: 'community', name })!
  // Substitute a harmless argv recorder for Depot; no catalog text becomes shell syntax.
  const recorded = execFileSync('/bin/sh', ['-c', `depot() { printf '%s\\n' "$#" "$1" "$2"; }; ${command}`], { encoding: 'utf8' })
  assert.equal(recorded, `2\nadd\ncommunity/${name}\n`)
})

test('display titles and incomplete or control-containing references are not install authority', () => {
  for (const value of [{ title: 'Display title' }, { namespace: 'community' }, { namespace: 'community', name: 'sample\nexec' }]) {
    assert.equal(artifactInstallCommand({ providerId: 'catalog', artifactId: 'id', ...value }), undefined)
  }
})
