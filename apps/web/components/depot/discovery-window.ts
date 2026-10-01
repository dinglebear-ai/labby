import type { FederatedArtifact } from '@/lib/api/depot-client'
import { artifactKey } from '@/lib/depot/provider-model'

const MAX_PAGES = 20
const MAX_ROWS = 1000
const MAX_BYTES = 8 * 1024 * 1024
const VISIBLE_PAGES = 3

export type CatalogWindow<T> = {
  pages: T[][]
  index: Map<string, T>
  pageBytes: number[]
  projectedBytes: number
  rowCount: number
  evictedRows: number
  historyExpired: boolean
}

export type DiscoveryWindow = CatalogWindow<FederatedArtifact>

export function createCatalogWindow<T>(): CatalogWindow<T> {
  return { pages: [], index: new Map(), pageBytes: [], projectedBytes: 0, rowCount: 0, evictedRows: 0, historyExpired: false }
}

function projectedSize(value: unknown): number {
  return new TextEncoder().encode(JSON.stringify(value)).length
}

export function appendCatalogPage<T>(current: CatalogWindow<T>, incoming: T[], keyFor: (item: T) => string): CatalogWindow<T> {
  const index = new Map(current.index)
  const page = incoming.filter(item => {
    const key = keyFor(item)
    if (index.has(key)) return false
    index.set(key, item)
    return true
  })
  if (page.length === 0) return { ...current, index }
  const pages = [...current.pages, page]
  const bytes = page.reduce((total, item) => total + projectedSize(item), 0)
  const pageBytes = [...current.pageBytes, bytes]
  let projectedBytes = current.projectedBytes + bytes
  let rowCount = current.rowCount + page.length
  let evictedRows = current.evictedRows
  while (pages.length > MAX_PAGES || rowCount > MAX_ROWS || projectedBytes > MAX_BYTES) {
    const removed = pages.shift()
    const removedBytes = pageBytes.shift() ?? 0
    if (!removed) break
    for (const item of removed) index.delete(keyFor(item))
    rowCount -= removed.length
    evictedRows += removed.length
    projectedBytes -= removedBytes
  }
  return { pages, index, pageBytes, projectedBytes, rowCount, evictedRows, historyExpired: evictedRows > 0 }
}

export function createDiscoveryWindow(): DiscoveryWindow {
  return createCatalogWindow<FederatedArtifact>()
}

export function appendDiscoveryPage(current: DiscoveryWindow, incoming: FederatedArtifact[]): DiscoveryWindow {
  return appendCatalogPage(current, incoming, item => artifactKey(item.providerId, item.artifactId))
}

export function visibleArtifacts(window: DiscoveryWindow, anchorPage = window.pages.length - 1) {
  const start = Math.max(0, Math.min(anchorPage - 1, Math.max(0, window.pages.length - VISIBLE_PAGES)))
  const selected = window.pages.slice(start, start + VISIBLE_PAGES)
  const leadingRows = window.pages.slice(0, start).reduce((sum, page) => sum + page.length, 0)
  const items = selected.flat()
  return { items, leadingRows, trailingRows: window.rowCount - leadingRows - items.length, startPage: start }
}
