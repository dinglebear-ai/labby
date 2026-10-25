import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { assertStaticNavigationBuildId } from './static-build-id-contract.mjs'

const payload = id => `:HL["/style.css","style"]\n0:${JSON.stringify({ b: id, t: { t: {} } })}\n`

test('static navigation checks the exact Next 16.4 root build ID', () => {
  assert.doesNotThrow(() => assertStaticNavigationBuildId('candidate-123', payload('candidate-123'), '<html></html>'))
})
test('missing, duplicate, malformed and incorrectly typed root records fail', () => {
  for (const tree of ['', '1:{"b":"candidate"}\n', '0:{broken}\n', '0:null\n', '0:[]\n', '0:{"b":3}\n', '0:{}\n', payload('candidate') + payload('candidate')]) {
    assert.throws(() => assertStaticNavigationBuildId('candidate', tree, '<html></html>'))
  }
})
test('a matching build ID elsewhere cannot hide a stale root build ID', () => {
  assert.throws(() => assertStaticNavigationBuildId('candidate', payload('stale') + '1:{"b":"candidate"}\n', ''), /current build ID/)
  assert.throws(() => assertStaticNavigationBuildId('candidate', '0:{"buildId":"candidate"}\n', ''), /string build ID/)
})
test('an empty expected build ID fails even when the payload matches', () => {
  assert.throws(() => assertStaticNavigationBuildId('', payload(''), ''), /non-empty/)
  assert.throws(() => assertStaticNavigationBuildId('   ', payload('   '), ''), /non-empty/)
})
test('server-coordinated deployment markers remain forbidden', () => {
  for (const html of ['<html data-dpl-id="x">', '<script src="/chunk.js?dpl=x">', '<script src="/chunk.js?a=1&dpl=x">']) {
    assert.throws(() => assertStaticNavigationBuildId('candidate', payload('candidate'), html), /deployment IDs/)
  }
})
test('the real gate CLI accepts matching exports and rejects stale navigation data', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'labby build id Ω '))
  try {
    await Promise.all(['scripts', '.next', 'out/loadouts'].map(name => mkdir(path.join(root, name), { recursive: true })))
    await Promise.all(['check-static-build-id.mjs', 'static-build-id-contract.mjs'].map(name => copyFile(new URL(name, import.meta.url), path.join(root, 'scripts', name))))
    await writeFile(path.join(root, '.next/BUILD_ID'), 'candidate\n')
    await writeFile(path.join(root, 'out/loadouts/index.html'), '<html></html>')
    await writeFile(path.join(root, 'out/loadouts/__next._tree.txt'), payload('candidate'))
    const run = () => spawnSync(process.execPath, [path.join(root, 'scripts/check-static-build-id.mjs')], { encoding: 'utf8', timeout: 10_000 })
    const valid = run()
    assert.equal(valid.status, 0, valid.stderr)
    assert.match(valid.stdout, /verified: candidate/)
    await writeFile(path.join(root, 'out/loadouts/__next._tree.txt'), payload('stale'))
    const stale = run()
    assert.equal(stale.status, 1)
    assert.match(stale.stderr, /current build ID/)
  } finally { await rm(root, { recursive: true, force: true }) }
})
