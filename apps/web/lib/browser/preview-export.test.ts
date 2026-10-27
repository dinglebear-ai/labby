import test, { type TestContext } from 'node:test'
import { spawn, type ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, rm, stat } from 'node:fs/promises'
import path from 'node:path'
import os from 'node:os'
import { publishPreviewExport, cleanupPreviewExport } from './preview-export.ts'

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


function within<T>(operation: Promise<T>, milliseconds: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout>
  const deadline = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`preview cleanup exceeded ${milliseconds}ms`)), milliseconds)
  })
  return Promise.race([operation, deadline]).finally(() => clearTimeout(timer))
}

async function ownedPreview(t: TestContext, ignoreTerm = false): Promise<{ child: ChildProcess; root: string }> {
  const root = await mkdtemp(path.join(os.tmpdir(), 'labby-preview-cleanup-'))
  const child = spawn(process.execPath, ['-e', `
    ${ignoreTerm ? "process.on('SIGTERM', () => {})" : ''}
    process.send('ready')
    setInterval(() => {}, 1000)
  `], { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] })
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) {
      const exited = once(child, 'exit')
      child.kill('SIGKILL')
      await within(exited, 2_000)
    }
    await rm(root, { recursive: true, force: true })
  })
  await within(once(child, 'message'), 3_000)
  return { child, root }
}

test('preview cleanup settles a SIGTERM exit and removes its owned export', { timeout: 10_000 }, async (t) => {
  const { child, root } = await ownedPreview(t)
  await within(cleanupPreviewExport(child, root), 500)
  assert.equal(child.exitCode, null)
  assert.equal(child.signalCode, 'SIGTERM')
  await assert.rejects(stat(root), { code: 'ENOENT' })
})

test('preview cleanup accepts an already signaled child without waiting for another exit', { timeout: 10_000 }, async (t) => {
  const { child, root } = await ownedPreview(t)
  const exited = once(child, 'exit')
  child.kill('SIGTERM')
  await within(exited, 2_000)
  await within(cleanupPreviewExport(child, root), 500)
  assert.equal(child.signalCode, 'SIGTERM')
  await assert.rejects(stat(root), { code: 'ENOENT' })
})

test('preview cleanup gives a resistant child the existing two-second grace then forces bounded exit', { timeout: 10_000 }, async (t) => {
  const { child, root } = await ownedPreview(t, true)
  const started = performance.now()
  await within(cleanupPreviewExport(child, root), 5_000)
  assert.ok(performance.now() - started >= 1_900, 'SIGTERM grace must remain two seconds')
  assert.equal(child.signalCode, 'SIGKILL')
  await assert.rejects(stat(root), { code: 'ENOENT' })
})

test('preview cleanup removes its owned export when no server was started', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'labby-preview-unstarted-'))
  try {
    await cleanupPreviewExport(null, root)
    await assert.rejects(stat(root), { code: 'ENOENT' })
  } finally { await rm(root, { recursive: true, force: true }) }
})
