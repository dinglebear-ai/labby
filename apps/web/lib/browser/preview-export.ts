import { type ChildProcess } from 'node:child_process'
import { cp, mkdtemp, rename, rm } from 'node:fs/promises'
import path from 'node:path'

/** Publish a task-owned copy so later builds cannot mutate served test assets. */
export async function publishPreviewExport(source: string, target: string): Promise<void> {
  const staging = await mkdtemp(`${target}-stage-`)
  const previous = `${staging}-previous`
  let movedPrevious = false
  try {
    await cp(source, staging, { recursive: true })
    try { await rename(target, previous); movedPrevious = true } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
    }
    try { await rename(staging, target) } catch (error) {
      if (movedPrevious) await rename(previous, target)
      throw error
    }
  } finally {
    await rm(staging, { recursive: true, force: true })
    if (movedPrevious) await rm(previous, { recursive: true, force: true })
  }
}

export function exportPath(root: string): string { return path.join(root, 'export') }

const PREVIEW_EXIT_GRACE_MS = 2_000

function previewExited(server: ChildProcess): boolean {
  return server.exitCode !== null || server.signalCode !== null
}

function signalPreviewAndWait(server: ChildProcess, signal: NodeJS.Signals): Promise<boolean> {
  if (previewExited(server)) return Promise.resolve(true)
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timer)
      server.off('exit', onExit)
      server.off('error', onError)
    }
    const onExit = () => { cleanup(); resolve(true) }
    const onError = (error: Error) => { cleanup(); reject(error) }
    const timer = setTimeout(() => { cleanup(); resolve(false) }, PREVIEW_EXIT_GRACE_MS)
    // Subscribe before signaling so an immediate exit cannot be missed.
    server.once('exit', onExit)
    server.once('error', onError)
    if (previewExited(server)) { onExit(); return }
    try { server.kill(signal) } catch (error) { cleanup(); reject(error); return }
    if (previewExited(server)) onExit()
  })
}

/** Stop only the owned direct child; retain the export if settlement fails. */
export async function cleanupPreviewExport(server: ChildProcess | null, root: string | null): Promise<void> {
  if (server && !await signalPreviewAndWait(server, 'SIGTERM')) {
    if (!await signalPreviewAndWait(server, 'SIGKILL')) {
      throw new Error('Owned browser preview did not exit after SIGKILL within 2000ms; export retained')
    }
  }
  if (root) await rm(root, { recursive: true, force: true })
}
