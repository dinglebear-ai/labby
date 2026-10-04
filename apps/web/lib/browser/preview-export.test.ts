import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import os from 'node:os'
import { publishPreviewExport } from './preview-export.ts'

test('browser preview exports remain isolated from subsequent builds and can publish intentional skew', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'labby-browser-export-'))
  try {
    const source = path.join(root, 'out'), target = path.join(root, 'preview')
    await mkdir(source)
    await writeFile(path.join(source, 'index.html'), 'mock build')
    await writeFile(path.join(source, 'old.js'), 'old asset')
    await publishPreviewExport(source, target)
    await writeFile(path.join(source, 'index.html'), 'production build')
    assert.equal(await readFile(path.join(target, 'index.html'), 'utf8'), 'mock build')
    await rm(path.join(source, 'old.js'))
    await publishPreviewExport(source, target)
    assert.equal(await readFile(path.join(target, 'index.html'), 'utf8'), 'production build')
    await assert.rejects(readFile(path.join(target, 'old.js')), { code: 'ENOENT' })
  } finally { await rm(root, { recursive: true, force: true }) }
})
