import assert from 'node:assert/strict'
import { createReadStream } from 'node:fs'
import { chmod, lstat, mkdtemp, readFile, realpath, rm, stat, writeFile } from 'node:fs/promises'
import path from 'node:path'

import type { Browser, BrowserContext, Page, Request } from 'playwright'

export const LIVE_DESCRIPTOR_ENV = 'LABBY_LIVE_BROWSER_DESCRIPTOR'
export const LIVE_DEADLINE_MS = 90_000
export const DEADLINE_DRAIN_GRACE_MS = 1_000
export const MAX_EVIDENCE_EVENTS = 512
export const MAX_EVIDENCE_TEXT_BYTES = 256 * 1024
export const MAX_ARTIFACT_BYTES = 16 * 1024 * 1024

export type LiveBackendDescriptor = {
  version: 1
  run_id: string
  base_url: string
  run_root: string
  storage_state_path: string
  csrf_state_path: string
  evidence_dir: string
  scan_secrets_path: string
  nightly?: boolean
}

// Supervisor contract for `.9`:
// - write this descriptor and Playwright storage state mode 0600 inside the
//   run-owned root; evidence_dir is mode 0700 and must also be run-owned;
// - serve prebuilt `apps/gateway-admin/out` from base_url and create the browser
//   session before invoking `pnpm run test:browser:live`;
// - set LABBY_LIVE_BROWSER_NIGHTLY=true only in the nightly job;
// - retain ownership of Labby, ports, credentials, fixtures, and teardown.

export type LiveBrowserEvidence = {
  requests: Array<{ method: string; path: string; status?: number }>
  console: string[]
  pageErrors: string[]
  failedRequests: string[]
  cspViolations: string[]
}

type CaptureOutcome =
  | { status: 'captured'; path: string }
  | { status: 'failed'; error: string }

function ownedAbsolutePath(value: unknown, field: string) {
  if (typeof value !== 'string') throw new TypeError(`${field} must be a string`)
  assert.ok(path.isAbsolute(value), `${field} must be absolute`)
  assert.ok(!value.includes('\0'), `${field} contains a NUL`)
  return path.resolve(value)
}

function isStrictDescendant(root: string, candidate: string) {
  const relative = path.relative(root, candidate)
  return relative !== '' && relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative)
}

async function canonicalOwnedPath(root: string, value: unknown, field: string, kind: 'file' | 'directory') {
  const canonicalRoot = await realpath(root)
  const resolved = ownedAbsolutePath(value, field)
  const metadata = await lstat(resolved)
  assert.ok(!metadata.isSymbolicLink(), `${field} must not be a symlink`)
  assert.equal(kind === 'file' ? metadata.isFile() : metadata.isDirectory(), true, `${field} must be a ${kind}`)
  const canonical = await realpath(resolved)
  assert.ok(isStrictDescendant(canonicalRoot, canonical), `${field} must be below the run-owned root`)
  return canonical
}

export async function readLiveDescriptor(): Promise<LiveBackendDescriptor | null> {
  const descriptorPath = process.env[LIVE_DESCRIPTOR_ENV]
  if (!descriptorPath) return null
  return readLiveDescriptorAt(descriptorPath)
}

export async function readLiveDescriptorAt(descriptorPath: string): Promise<LiveBackendDescriptor> {
  const unresolvedDescriptor = ownedAbsolutePath(descriptorPath, LIVE_DESCRIPTOR_ENV)
  const descriptorMetadata = await lstat(unresolvedDescriptor)
  assert.ok(!descriptorMetadata.isSymbolicLink(), 'descriptor must not be a symlink')
  assert.ok(descriptorMetadata.isFile(), 'descriptor must be a file')
  const resolved = await realpath(unresolvedDescriptor)
  const parsed = JSON.parse(await readFile(resolved, 'utf8')) as Partial<LiveBackendDescriptor>
  assert.equal(parsed.version, 1)
  assert.match(parsed.run_id ?? '', /^[A-Za-z0-9_-]{8,80}$/)
  const unresolvedRoot = ownedAbsolutePath(parsed.run_root, 'run_root')
  const rootMetadata = await lstat(unresolvedRoot)
  assert.ok(!rootMetadata.isSymbolicLink() && rootMetadata.isDirectory(), 'run_root must be a real directory')
  const runRoot = await realpath(unresolvedRoot)
  assert.ok(isStrictDescendant(runRoot, resolved), 'descriptor must be below the run-owned root')
  const url = new URL(parsed.base_url ?? '')
  assert.equal(url.protocol, 'http:', 'live browser backend must be loopback HTTP')
  assert.ok(['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname))
  assert.equal(url.username, '')
  assert.equal(url.password, '')
  assert.equal(url.pathname, '/')
  const evidenceDir = await canonicalOwnedPath(runRoot, parsed.evidence_dir, 'evidence_dir', 'directory')
  const storageStatePath = await canonicalOwnedPath(runRoot, parsed.storage_state_path, 'storage_state_path', 'file')
  const csrfStatePath = await canonicalOwnedPath(runRoot, parsed.csrf_state_path, 'csrf_state_path', 'file')
  const descriptorStat = await stat(resolved)
  const storageStateStat = await stat(storageStatePath)
  const csrfStateStat = await stat(csrfStatePath)
  assert.equal(descriptorStat.mode & 0o077, 0, 'descriptor must be mode 0600')
  assert.equal(storageStateStat.mode & 0o077, 0, 'storage state must be mode 0600')
  assert.equal(csrfStateStat.mode & 0o077, 0, 'CSRF state must be mode 0600')
  assert.ok(storageStatePath !== resolved, 'descriptor cannot double as credential state')
  const scanSecretsPath = await canonicalOwnedPath(runRoot, parsed.scan_secrets_path, 'scan_secrets_path', 'file')
  const scanSecretsStat = await stat(scanSecretsPath)
  assert.equal(scanSecretsStat.mode & 0o077, 0, 'scan secrets must be mode 0600')
  return {
    version: 1,
    run_id: parsed.run_id!,
    base_url: url.toString().replace(/\/$/, ''),
    run_root: runRoot,
    storage_state_path: storageStatePath,
    csrf_state_path: csrfStatePath,
    evidence_dir: evidenceDir,
    scan_secrets_path: scanSecretsPath,
    nightly: parsed.nightly === true,
  }
}

export async function readPrivateCsrf(descriptor: LiveBackendDescriptor) {
  const value = JSON.parse(await readFile(descriptor.csrf_state_path, 'utf8')) as { csrf_token?: unknown }
  if (typeof value.csrf_token !== 'string') throw new TypeError('csrf_token must be a string')
  assert.ok(value.csrf_token.length >= 16)
  return value.csrf_token
}

function safePathname(raw: string, baseUrl: string) {
  const url = new URL(raw, baseUrl)
  return url.origin === new URL(baseUrl).origin ? `${url.pathname}${url.search}` : '[cross-origin]'
}

export function observeLivePage(page: Page, baseUrl: string): LiveBrowserEvidence {
  const evidence: LiveBrowserEvidence = {
    requests: [], console: [], pageErrors: [], failedRequests: [], cspViolations: [],
  }
  const pending = new WeakMap<Request, number>()
  let retainedBytes = 0
  const retain = (values: string[], value: string) => {
    const bytes = Buffer.byteLength(value)
    if (values.length >= MAX_EVIDENCE_EVENTS || retainedBytes + bytes > MAX_EVIDENCE_TEXT_BYTES) return
    values.push(value)
    retainedBytes += bytes
  }
  page.on('request', (request) => {
    if (!request.url().startsWith(baseUrl)) return
    if (evidence.requests.length >= MAX_EVIDENCE_EVENTS) return
    pending.set(request, evidence.requests.push({ method: request.method(), path: safePathname(request.url(), baseUrl) }) - 1)
  })
  page.on('response', (response) => {
    const index = pending.get(response.request())
    if (index !== undefined) evidence.requests[index]!.status = response.status()
  })
  page.on('requestfailed', (request) => retain(evidence.failedRequests, `${request.method()} ${safePathname(request.url(), baseUrl)}`))
  page.on('console', (message) => {
    const rendered = `${message.type()}: ${message.text()}`
    retain(evidence.console, rendered)
    if (/content security policy|csp/i.test(rendered)) retain(evidence.cspViolations, rendered)
  })
  page.on('pageerror', (error) => retain(evidence.pageErrors, error.message))
  return evidence
}

export function assertCanaryFree(value: unknown, canaries: string[], label: string) {
  const rendered = typeof value === 'string' ? value : JSON.stringify(value)
  for (const canary of canaries) assert.ok(!rendered.includes(canary), `${label} leaked a secret canary`)
  assert.ok(!/authorization\s*[:=]\s*bearer/i.test(rendered), `${label} leaked bearer metadata`)
  assert.ok(!/x-csrf-token\s*[:=]/i.test(rendered), `${label} leaked CSRF metadata`)
}

export async function withAbsoluteDeadline<T>(
  operation: (signal: AbortSignal) => Promise<T>,
  label: string,
  timeoutMs = LIVE_DEADLINE_MS,
  drainGraceMs = DEADLINE_DRAIN_GRACE_MS,
): Promise<T> {
  const controller = new AbortController()
  const running = Promise.resolve().then(() => operation(controller.signal))
  let timer: NodeJS.Timeout | undefined
  let timedOut = false
  try {
    const value = await Promise.race([
      running,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          timedOut = true
          controller.abort(new Error(`${label} exceeded ${timeoutMs}ms`))
          reject(controller.signal.reason)
        }, timeoutMs)
      }),
    ])
    return value
  } finally {
    if (timer) clearTimeout(timer)
    if (timedOut) {
      // Keep a rejection handler attached even when an operation ignores its
      // AbortSignal. Cleanup gets a short grace period, but a non-cooperative
      // dependency cannot extend the caller's absolute deadline forever.
      const settled = running.catch(() => undefined)
      let drainTimer: NodeJS.Timeout | undefined
      await Promise.race([
        settled,
        new Promise<void>((resolve) => { drainTimer = setTimeout(resolve, drainGraceMs) }),
      ])
      if (drainTimer) clearTimeout(drainTimer)
    }
  }
}

export async function launchBrowserWithAbort<T extends Pick<Browser, 'close'>>(
  signal: AbortSignal,
  launch: () => Promise<T>,
): Promise<{ browser: T; detachAbort: () => void }> {
  const browser = await launch()
  let closePromise: Promise<void> | undefined
  const closeBrowser = () => {
    closePromise ??= browser.close().then(() => undefined, () => undefined)
    return closePromise
  }
  const abortBrowser = () => { void closeBrowser() }
  signal.addEventListener('abort', abortBrowser, { once: true })

  // An abort can occur while launch() is pending. Registering the listener
  // after launch is not sufficient because AbortSignal does not replay events.
  if (signal.aborted) {
    await closeBrowser()
    signal.throwIfAborted()
  }
  return {
    browser,
    detachAbort: () => signal.removeEventListener('abort', abortBrowser),
  }
}

export async function captureFailureEvidence(options: {
  browser: Browser
  context: BrowserContext
  page: Page
  descriptor: LiveBackendDescriptor
  evidence: LiveBrowserEvidence
  error: unknown
}) {
  const { context, page, descriptor, evidence } = options
  const evidenceDir = await canonicalOwnedPath(descriptor.run_root, descriptor.evidence_dir, 'evidence_dir', 'directory')
  const invocationDir = await mkdtemp(path.join(evidenceDir, `browser-${descriptor.run_id}-`))
  await chmod(invocationDir, 0o700)
  const prefix = path.join(invocationDir, 'failure')
  const screenshot = `${prefix}.png`
  const trace = `${prefix}.trace.zip`
  const screenshotOutcome: CaptureOutcome = await page.screenshot({ path: screenshot, fullPage: true })
    .then(() => ({ status: 'captured' as const, path: screenshot }))
    .catch((error: unknown) => ({ status: 'failed' as const, error: renderError(error) }))
  const traceOutcome: CaptureOutcome = await context.tracing.stop({ path: trace })
    .then(() => ({ status: 'captured' as const, path: trace }))
    .catch((error: unknown) => ({ status: 'failed' as const, error: renderError(error) }))
  const report = {
    run_id: descriptor.run_id,
    error: options.error instanceof Error ? options.error.message : String(options.error),
    evidence,
    captures: { screenshot: screenshotOutcome, trace: traceOutcome },
  }
  const reportPath = `${prefix}.json`
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 })
  const secrets = (await readFile(descriptor.scan_secrets_path, 'utf8'))
    .split('\n').filter(Boolean).map((value) => Buffer.from(value))
  assert.ok(secrets.length > 0, 'scan-only secret set must not be empty')
  const artifacts = [
    reportPath,
    ...(screenshotOutcome.status === 'captured' ? [screenshotOutcome.path] : []),
    ...(traceOutcome.status === 'captured' ? [traceOutcome.path] : []),
  ]
  try {
    for (const artifact of artifacts) await scanArtifact(artifact, secrets)
  } catch (error) {
    // This invocation owns only its mkdtemp directory. Never traverse or
    // remove sibling evidence, even if a decoy was present before the run.
    await rm(invocationDir, { force: true, recursive: true })
    throw error
  }
  const captureFailures = [screenshotOutcome, traceOutcome]
    .filter((outcome): outcome is Extract<CaptureOutcome, { status: 'failed' }> => outcome.status === 'failed')
  if (captureFailures.length > 0) {
    throw new AggregateError(
      captureFailures.map((outcome) => new Error(outcome.error)),
      'browser failure evidence capture was incomplete',
    )
  }
}

function renderError(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export async function scanArtifact(artifact: string, secrets: Buffer[], optional = false) {
  let metadata: Awaited<ReturnType<typeof stat>>
  try {
    metadata = await stat(artifact)
  } catch (error) {
    if (optional && (error as NodeJS.ErrnoException).code === 'ENOENT') return
    throw error
  }
  if (metadata.size > MAX_ARTIFACT_BYTES) throw new Error(`${path.basename(artifact)} exceeded artifact cap`)
  const overlap = Math.max(...secrets.map((secret) => secret.length), 1) - 1
  let tail = Buffer.alloc(0)
  for await (const chunk of createReadStream(artifact, { highWaterMark: 64 * 1024 })) {
    const combined = Buffer.concat([tail, chunk as Buffer])
    if (secrets.some((secret) => combined.includes(secret))) {
      throw new Error(`${path.basename(artifact)} contained scan-only secret material`)
    }
    tail = overlap === 0 ? Buffer.alloc(0) : combined.subarray(Math.max(0, combined.length - overlap))
  }
}
