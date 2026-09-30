/**
 * Workspace filesystem browser HTTP client.
 *
 * Thin fetch wrappers around `GET /v1/fs/list` and `GET /v1/fs/preview`.
 * The preview returns a `Blob`; creating an object URL from that blob is
 * acceptable because the bytes are backend-approved (deny-list + size-cap)
 * — unlike user-supplied `File` blobs, which were the banned pattern from
 * the original lab-f1t2 review.
 */

import type { FsListResponse } from './types'

/** Error thrown for non-2xx responses; exposes the backend envelope. */
export class FsClientError extends Error {
  readonly status: number
  readonly kind: string | undefined
  readonly envelope: unknown

  constructor(status: number, envelope: unknown, message: string) {
    super(message)
    this.name = 'FsClientError'
    this.status = status
    this.envelope = envelope
    this.kind =
      envelope && typeof envelope === 'object' && 'kind' in envelope
        ? String((envelope as { kind: unknown }).kind)
        : undefined
  }
}

async function parseError(res: Response): Promise<FsClientError> {
  let envelope: unknown = null
  try {
    envelope = await res.json()
  } catch {
    // body wasn't JSON — leave envelope null
  }
  let message = `fs request failed with status ${res.status}`
  if (envelope && typeof envelope === 'object' && 'message' in envelope) {
    const raw = (envelope as { message: unknown }).message
    if (typeof raw === 'string' && raw.length > 0) {
      message = raw
    }
  }
  return new FsClientError(res.status, envelope, message)
}

/**
 * List the immediate entries of a workspace directory. Empty or undefined
 * `path` lists the workspace root. Credential-deny-listed entries are
 * filtered server-side and never returned.
 */
export async function listWorkspace(
  path?: string,
  init?: { signal?: AbortSignal },
): Promise<FsListResponse> {
  const url = new URL('/v1/fs/list', window.location.origin)
  if (path && path.length > 0) {
    url.searchParams.set('path', path)
  }
  const res = await fetch(url.toString(), {
    method: 'GET',
    credentials: 'same-origin',
    signal: init?.signal,
  })
  if (!res.ok) {
    throw await parseError(res)
  }
  return (await res.json()) as FsListResponse
}

/**
 * Module-level in-flight cache for `previewWorkspaceFile`. Keyed by
 * `path|maxBytes` so range/cap variations don't alias. Values are the
 * underlying fetch promise — once settled, the entry is removed (see
 * eviction strategy below).
 *
 * Why this exists: when a chat input has N image attachments, each
 * `AttachmentChip` mounts and fires its own `previewWorkspaceFile`
 * concurrently. Without dedupe that's N parallel streams of up to 2 MiB
 * each. Re-mount churn (chip removed and re-added for the same path) also
 * re-fetches. The cache collapses concurrent identical requests into one.
 *
 * Eviction strategy: entry is cleared on settle (success OR failure) via
 * `.finally`. This means a successful fetch is NOT memoized across time —
 * a later caller after settle will re-fetch. That's intentional: blobs
 * can be large (≤2 MiB each), and holding them in a module-level Map
 * would leak memory across navigations. The dedupe goal is "concurrent
 * fanout", not "long-lived cache". On error this also lets the next
 * caller retry instead of seeing a stuck rejection.
 *
 * Abort semantics trade-off: the shared in-flight fetch IGNORES per-caller
 * `AbortSignal`. Once two callers share a fetch, one caller's abort cannot
 * cancel the network operation without harming the other. The simpler
 * choice (taken here) is to run the shared fetch to completion without a
 * signal; per-caller cancellation is observed only at the await boundary
 * (the caller checks its own `signal.aborted` after `await`). The bytes
 * still arrive in memory, but unmounted callers discard them and the
 * cache entry is freed on settle. Callers that need hard mid-flight
 * cancellation must avoid the dedupe path (not currently exposed).
 */
const previewInFlight = new Map<string, Promise<{ blob: Blob; contentType: string }>>()

/** Visible for testing — clear cache state between tests. */
export function __resetPreviewCache(): void {
  previewInFlight.clear()
}

function previewCacheKey(path: string, maxBytes: number | undefined): string {
  return `${path}|${maxBytes ?? ''}`
}

/**
 * Internal: perform the actual streaming fetch without dedupe. Honors the
 * supplied signal mid-stream via the same getReader() loop documented on
 * the public function.
 */
async function fetchPreview(
  path: string,
  options?: { maxBytes?: number; signal?: AbortSignal },
): Promise<{ blob: Blob; contentType: string }> {
  const url = new URL('/v1/fs/preview', window.location.origin)
  url.searchParams.set('path', path)
  if (options?.maxBytes !== undefined) {
    url.searchParams.set('max_bytes', String(options.maxBytes))
  }
  const res = await fetch(url.toString(), {
    method: 'GET',
    credentials: 'same-origin',
    signal: options?.signal,
  })
  if (!res.ok) {
    throw await parseError(res)
  }
  const contentType = res.headers.get('content-type') ?? 'application/octet-stream'
  // Stream the body via getReader() rather than res.blob(). res.blob() reads
  // the stream to completion without honoring AbortSignal mid-flight, so a
  // rapid attach/remove holds bytes in memory until the full body lands.
  // With a manual reader loop we can check AbortSignal between chunks and
  // bail early — releasing the underlying connection and freeing buffers.
  if (!res.body) {
    // No streaming body available; fall back to the one-shot blob read.
    const blob = await res.blob()
    return { blob, contentType }
  }
  const reader = res.body.getReader()
  const chunks: Uint8Array[] = []
  try {
    while (true) {
      if (options?.signal?.aborted) {
        // DOMException name 'AbortError' matches what fetch itself throws on
        // signal abort, so callers (e.g. AttachmentChip) can treat both
        // identically with a single .catch.
        throw new DOMException('preview aborted', 'AbortError')
      }
      const { done, value } = await reader.read()
      if (done) break
      if (value) chunks.push(value)
    }
  } finally {
    // cancel() is a no-op once the stream is fully read; safe to always call
    // so an aborted loop releases the underlying connection promptly.
    void reader.cancel().catch(() => {})
  }
  const blob = new Blob(chunks as BlobPart[], { type: contentType })
  return { blob, contentType }
}

/**
 * Stream a capped byte window from a workspace file. Returned as a `Blob`
 * so callers can render it via `URL.createObjectURL(blob)` for thumbnails
 * (revoke on unmount). `max_bytes` is a hint — the server clamps at 2 MiB
 * regardless.
 *
 * Concurrent identical requests (same path + maxBytes) share a single
 * in-flight fetch — see `previewInFlight` above for the trade-offs. If
 * the caller passes a `signal`, abort is observed at the await boundary
 * via a post-await check; the underlying shared fetch is NOT cancelled
 * because other subscribers may still need its result.
 */
export async function previewWorkspaceFile(
  path: string,
  options?: { maxBytes?: number; signal?: AbortSignal },
): Promise<{ blob: Blob; contentType: string }> {
  const key = previewCacheKey(path, options?.maxBytes)
  let pending = previewInFlight.get(key)
  if (!pending) {
    // Start the shared fetch WITHOUT the caller's signal — once another
    // subscriber joins, one caller's abort must not cancel the network
    // operation for everyone. Subscribers observe their own abort below
    // by re-checking signal.aborted after the await.
    pending = fetchPreview(path, { maxBytes: options?.maxBytes })
    previewInFlight.set(key, pending)
    // Always remove on settle so failed fetches can be retried and
    // successful results don't leak in this module-level Map. Use a
    // separate `.finally` chain so we don't observe rejection here
    // (subscribers below get the rejection on their own await).
    pending
      .finally(() => {
        // Only clear if still the same entry — defensive in case a future
        // refactor reseeds the slot.
        if (previewInFlight.get(key) === pending) {
          previewInFlight.delete(key)
        }
      })
      // Swallow rejection on this side channel — subscribers below observe
      // it via their own await. Without this, the .finally chain produces
      // an unobserved rejected promise and Node fires an unhandled-rejection.
      .catch(() => {})
  }
  // Wait for the shared fetch. If our caller aborted while waiting, throw
  // an AbortError so downstream `.catch` paths behave the same as the
  // non-deduped path. The shared fetch keeps running for other subscribers.
  const result = await pending
  if (options?.signal?.aborted) {
    throw new DOMException('preview aborted', 'AbortError')
  }
  return result
}

/** MIME types the picker will render as an inline thumbnail. */
export const INLINE_IMAGE_MIMES: ReadonlySet<string> = new Set([
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
])

export function isInlineImageMime(contentType: string): boolean {
  const base = contentType.split(';')[0]?.trim().toLowerCase() ?? ''
  return INLINE_IMAGE_MIMES.has(base)
}
