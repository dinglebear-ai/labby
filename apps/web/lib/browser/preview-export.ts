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
