import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, readdir, realpath, symlink, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'

import type { Browser, BrowserContext, Page } from 'playwright'

import {
  captureFailureEvidence,
  readLiveDescriptorAt,
  scanArtifact,
  type LiveBackendDescriptor,
  type LiveBrowserEvidence,
} from './live-backend-harness.ts'

async function privateFile(filePath: string, value: string) {
  await writeFile(filePath, value, { mode: 0o600 })
  return filePath
}

async function descriptorFixture() {
  const parent = await mkdtemp(path.join(os.tmpdir(), 'labby-browser-descriptor-'))
  const runRoot = path.join(parent, 'owned-run')
  const evidenceDir = path.join(runRoot, 'evidence')
  await mkdir(evidenceDir, { recursive: true, mode: 0o700 })
  const storageState = await privateFile(path.join(runRoot, 'storage.json'), '{}')
  const csrfState = await privateFile(path.join(runRoot, 'csrf.json'), '{"csrf_token":"0123456789abcdef"}')
  const scanSecrets = await privateFile(path.join(runRoot, 'scan-secrets'), 'secret-canary\n')
  const descriptorPath = path.join(runRoot, 'descriptor.json')
  const descriptor: LiveBackendDescriptor = {
    version: 1,
    run_id: 'browser_test_run',
    base_url: 'http://127.0.0.1:40123',
    run_root: runRoot,
    storage_state_path: storageState,
    csrf_state_path: csrfState,
    evidence_dir: evidenceDir,
    scan_secrets_path: scanSecrets,
  }
  await privateFile(descriptorPath, JSON.stringify(descriptor))
  return { parent, runRoot, evidenceDir, descriptorPath, descriptor }
}

test('live descriptor accepts only canonical paths below its run-owned root', async () => {
  const fixture = await descriptorFixture()
  const parsed = await readLiveDescriptorAt(fixture.descriptorPath)
  assert.equal(parsed.run_root, await realpath(fixture.runRoot))
  assert.equal(parsed.evidence_dir, await realpath(fixture.evidenceDir))

  const outside = await privateFile(path.join(fixture.parent, 'outside.json'), '{}')
  await privateFile(fixture.descriptorPath, JSON.stringify({ ...fixture.descriptor, storage_state_path: outside }))
  await assert.rejects(readLiveDescriptorAt(fixture.descriptorPath), /below the run-owned root/)
})

test('live descriptor rejects symlink leaves and symlinked path components', async () => {
  const fixture = await descriptorFixture()
  const outside = await privateFile(path.join(fixture.parent, 'outside.json'), '{}')
  const leafLink = path.join(fixture.runRoot, 'linked-storage.json')
  await symlink(outside, leafLink)
  await privateFile(fixture.descriptorPath, JSON.stringify({ ...fixture.descriptor, storage_state_path: leafLink }))
  await assert.rejects(readLiveDescriptorAt(fixture.descriptorPath), /must not be a symlink/)

  const linkedDirectory = path.join(fixture.runRoot, 'linked-directory')
  await symlink(fixture.parent, linkedDirectory)
  await privateFile(fixture.descriptorPath, JSON.stringify({
    ...fixture.descriptor,
    storage_state_path: path.join(linkedDirectory, 'outside.json'),
  }))
  await assert.rejects(readLiveDescriptorAt(fixture.descriptorPath), /symlink components|below the run-owned root/)
})

test('failed secret scan deletes only invocation-created evidence and preserves decoys', async () => {
  const fixture = await descriptorFixture()
  const decoy = path.join(fixture.evidenceDir, 'preexisting-decoy.txt')
  await privateFile(decoy, 'must survive')
  const evidence: LiveBrowserEvidence = {
    requests: [], console: [], pageErrors: [], failedRequests: [], cspViolations: [],
  }
  const page = {
    screenshot: async ({ path: screenshotPath }: { path: string }) => {
      await privateFile(screenshotPath, 'secret-canary')
    },
  } as unknown as Page
  const context = {
    tracing: { stop: async ({ path: tracePath }: { path: string }) => privateFile(tracePath, 'safe trace') },
  } as unknown as BrowserContext

  await assert.rejects(captureFailureEvidence({
    browser: {} as Browser,
    context,
    page,
    descriptor: fixture.descriptor,
    evidence,
    error: new Error('expected failure'),
  }), /contained scan-only secret material/)
  assert.equal(await readFile(decoy, 'utf8'), 'must survive')
})

test('capture failures are recorded in the retained report and reject the operation', async () => {
  const fixture = await descriptorFixture()
  const evidence: LiveBrowserEvidence = {
    requests: [], console: [], pageErrors: [], failedRequests: [], cspViolations: [],
  }
  const page = {
    screenshot: async () => { throw new Error('screenshot backend unavailable') },
  } as unknown as Page
  const context = {
    tracing: { stop: async () => { throw new Error('trace already stopped') } },
  } as unknown as BrowserContext

  await assert.rejects(captureFailureEvidence({
    browser: {} as Browser,
    context,
    page,
    descriptor: fixture.descriptor,
    evidence,
    error: new Error('journey failed'),
  }), /browser failure evidence capture was incomplete/)

  const [invocation] = await readdir(fixture.evidenceDir)
  assert.ok(invocation)
  const report = JSON.parse(await readFile(path.join(fixture.evidenceDir, invocation, 'failure.json'), 'utf8'))
  assert.deepEqual(report.captures, {
    screenshot: { status: 'failed', error: 'screenshot backend unavailable' },
    trace: { status: 'failed', error: 'trace already stopped' },
  })
})

test('artifact scanning tolerates only an explicitly optional missing file', async () => {
  const fixture = await descriptorFixture()
  const missing = path.join(fixture.runRoot, 'missing-artifact')
  await assert.rejects(scanArtifact(missing, [Buffer.from('secret-canary')]), { code: 'ENOENT' })
  await assert.doesNotReject(scanArtifact(missing, [Buffer.from('secret-canary')], true))
  await assert.rejects(scanArtifact(fixture.evidenceDir, [Buffer.from('secret-canary')]), /EISDIR|illegal operation/)
})

test('CI uploads only the exclusive current run-attempt evidence directory', async () => {
  const workflowPath = path.resolve(import.meta.dirname, '../../../../.github/workflows/ci.yml')
  const workflow = await readFile(workflowPath, 'utf8')
  assert.match(workflow, /run_root="\$\{RUNNER_TEMP\}\/labby-live-e2e-\$\{GITHUB_RUN_ID\}-\$\{GITHUB_RUN_ATTEMPT\}-\$\{GITHUB_JOB\}"/)
  assert.match(workflow, /path: \$\{\{ runner\.temp \}\}\/labby-live-e2e-\$\{\{ github\.run_id \}\}-\$\{\{ github\.run_attempt \}\}-live-e2e-core\/artifacts\//)
  assert.doesNotMatch(workflow, /\/tmp\/labby-live-e2e\.\*\/artifacts/)
})
