import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { appendFileSync } from 'node:fs'
import { access, readFile, writeFile } from 'node:fs/promises'
import test from 'node:test'

import { chromium, type Page } from 'playwright'

import {
  assertCanaryFree,
  captureFailureEvidence,
  observeLivePage,
  ownedBrowserLaunchOptions,
  readPrivateCsrf,
  readScanSecrets,
  readLiveDescriptor,
  runBrowserCleanupIfActive,
  useBrowserWithAbort,
  withAbsoluteDeadline,
} from './live-backend-harness.ts'

const liveEnabled = Boolean(process.env.LABBY_LIVE_BROWSER_DESCRIPTOR)
const nightlyEnabled = process.env.LABBY_LIVE_BROWSER_NIGHTLY === 'true'
const progressPath = process.env.LABBY_LIVE_BROWSER_PROGRESS
let progressBytes = 0
function progress(message: string) {
  const rendered = `${message}\n`
  progressBytes += Buffer.byteLength(rendered)
  if (progressPath && progressBytes <= 16 * 1024) appendFileSync(progressPath, rendered, { mode: 0o600 })
}

async function action(page: Page, csrfToken: string, service: string, name: string, params: object, teamId?: string) {
  return page.evaluate(async ({ csrfToken, service, name, params, teamId }) => {
    const session = await fetch('/auth/session', { credentials: 'include', cache: 'no-store' })
    const sessionBody = await session.json()
    if (!session.ok || !sessionBody.authenticated) return { status: session.status, body: sessionBody }
    const response = await fetch(`/v1/${service}`, {
      method: 'POST', credentials: 'include', cache: 'no-store',
      headers: {
        'content-type': 'application/json',
        'x-csrf-token': csrfToken,
        ...(teamId ? { 'x-labby-team-id': teamId } : typeof sessionBody.project_id === 'string' ? { 'x-labby-project-id': sessionBody.project_id } : {}),
      },
      body: JSON.stringify({ action: name, params }),
    })
    return {
      status: response.status,
      body: await response.json(),
      sessionAuthenticated: sessionBody.authenticated === true,
      csrfLength: typeof sessionBody.csrf_token === 'string' ? sessionBody.csrf_token.length : 0,
      sessionProjectId: typeof sessionBody.project_id === 'string' ? sessionBody.project_id : null,
      sessionTeams: Array.isArray(sessionBody.teams) ? sessionBody.teams : [],
    }
  }, { csrfToken, service, name, params, teamId })
}

async function exerciseSnippetReplayThroughUi(page: Page, csrfToken: string, name: string) {
  const contents = 'owned browser replay artifact\n'
  const digest = createHash('sha256').update(contents).digest('hex')
  const body = `---\nname: ${name}\ndescription: Owned live browser replay regression\ntools: []\n---\n\`\`\`js\nasync () => { await writeArtifact('report.txt', ${JSON.stringify(contents)}, {contentType:'text/plain'}); return {ok:true}; }\n\`\`\`\n`
  const created = await action(page, csrfToken, 'snippets', 'snippets.create', { name, body })
  assert.equal(created.status, 200, 'owned live snippet must be created')
  try {
    const seed = await action(page, csrfToken, 'snippets', 'snippets.exec', { name, params: {} })
    assert.equal(seed.status, 200, 'seed must execute through the native runtime')
    assert.equal(seed.body.receipt_status, 'persisted')
    const seedId = seed.body.execution_id
    assert.equal(typeof seedId, 'string')
    await page.goto('/snippets/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
    await page.getByLabel('Search snippets', { exact: true }).fill(name)
    await page.getByText(name, { exact: true }).first().click()
    const history = page.getByRole('region', { name: `Run history for ${name}`, exact: true })
    await history.getByRole('button').filter({ hasText: seedId }).click()
    await history.getByRole('button', { name: 'Preview replay', exact: true }).click()
    const preview = page.getByRole('dialog', { name: `Replay preview · ${name}`, exact: true })
    const run = preview.getByRole('button', { name: 'Run now', exact: true })
    await run.waitFor({ state: 'visible', timeout: 10_000 })
    // Only acknowledge drift actually reported by this native preview.
    const acknowledgment = preview.getByRole('checkbox')
    if (await acknowledgment.count()) await acknowledgment.check()
    const replayResponse = page.waitForResponse(response => response.request().method() === 'POST'
      && new URL(response.url()).pathname === '/v1/snippets'
      && response.request().postDataJSON()?.action === 'snippets.replay', { timeout: 15_000 })
    await run.click({ timeout: 10_000 })
    const response = await replayResponse
    assert.equal(response.status(), 200, 'UI replay must complete against the real native backend')
    const replay = await response.json()
    assert.equal(replay.receipt_status, 'persisted')
    assert.equal(typeof replay.execution_id, 'string')
    assert.notEqual(replay.execution_id, seedId, 'replay creates a new retained execution')
    const request = response.request().postDataJSON()
    assert.equal(request.params.execution_id, seedId)
    assert.ok(request.params.expected_preview_fingerprint)
    await history.getByRole('button').filter({ hasText: replay.execution_id }).click({ timeout: 10_000 })
    const receipt = await action(page, csrfToken, 'snippets', 'snippets.receipt', { execution_id: replay.execution_id })
    assert.equal(receipt.status, 200)
    assert.equal(receipt.body.status, 'succeeded')
    assert.equal(receipt.body.artifacts.length, 1)
    assert.equal(receipt.body.artifacts[0].sha256, digest)
    assert.equal(receipt.body.artifacts[0].bytes, Buffer.byteLength(contents))
    const downloading = page.waitForEvent('download', { timeout: 10_000 })
    await history.getByRole('button', { name: 'Download artifact', exact: true }).click()
    const download = await downloading
    assert.equal(download.suggestedFilename(), 'report.txt')
    const downloadedPath = await download.path()
    assert.ok(downloadedPath)
    const bytes = await readFile(downloadedPath)
    assert.equal(bytes.toString('utf8'), contents)
    assert.equal(createHash('sha256').update(bytes).digest('hex'), digest)
    await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
    await page.getByLabel('Search snippets', { exact: true }).fill(name)
    await page.getByText(name, { exact: true }).first().click()
    await history.getByRole('button').filter({ hasText: replay.execution_id }).waitFor({ state: 'visible', timeout: 10_000 })
    progress('snippet-native-replay-history-download-reload')
  } finally {
    const removed = await action(page, csrfToken, 'snippets', 'snippets.remove', { name })
    assert.equal(removed.status, 200, 'owned browser snippet cleanup must succeed')
  }
}

async function addGatewayThroughUi(page: Page, name: string) {
  const observedPosts: string[] = []
  const observePost = (request: import('playwright').Request) => {
    if (request.method() === 'POST') observedPosts.push(`${new URL(request.url()).pathname}:${request.postData() ?? ''}`)
  }
  page.on('request', observePost)
  await page.getByRole('button', { name: 'Gateway actions, search and filters', exact: true }).click()
  await page.getByRole('menuitem', { name: 'Add server', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Add server' })
  await dialog.getByLabel('Name').fill(name)
  await dialog.getByLabel('URL').fill('http://127.0.0.1:9/mcp')
  const mutation = page.waitForResponse((response) =>
    response.request().method() === 'POST'
      && response.request().postData()?.includes('gateway.add') === true,
  { timeout: 30_000 })
  await dialog.getByRole('button', { name: 'Add server', exact: true }).click()
  const response = await mutation.catch(async (error) => {
    throw new Error(`add-server request was not sent; posts=${JSON.stringify(observedPosts)}; page=${await page.locator('body').innerText()}`, { cause: error })
  }).finally(() => page.off('request', observePost))
  assert.equal(response.status(), 200, `UI gateway.add returned ${response.status()}: ${await response.text()}`)
  await dialog.waitFor({ state: 'hidden', timeout: 10_000 }).catch(async (error) => {
    throw new Error(`add-server dialog remained open: ${await dialog.innerText()}`, { cause: error })
  })
  const serverLink = page.locator('a:visible').filter({ hasText: name }).first()
  await assert.doesNotReject(serverLink.waitFor({ state: 'visible', timeout: 10_000 }))
  const href = await serverLink.getAttribute('href')
  const id = new URL(href ?? '', 'http://loopback.invalid').searchParams.get('id')
  assert.ok(id, 'new server link must expose its stable identifier')
  return id
}

async function selectWorkspaceThroughUi(page: Page, teamId?: string) {
  await page.getByRole('button', { name: 'Switch workspace', exact: true }).click()
  const menu = page.locator('[data-scopemenu] [data-anim="menu"]')
  await menu.getByRole('button').filter({ has: page.getByText(teamId ?? 'Personal', { exact: true }) }).click()
  await menu.waitFor({ state: 'hidden', timeout: 10_000 })
  await page.waitForURL(url => url.pathname === '/', { timeout: 15_000 })
}

async function stageProtectedRouteThroughUi(page: Page, name: string, loadout: string) {
  // Keep the selected Team in the live app session through client navigation.
  await page.getByRole('button', { name: 'Account menu', exact: true }).click()
  await page.getByRole('link', { name: 'Settings', exact: true }).click()
  await page.getByRole('link', { name: /^Client access/ }).click()
  const panel = page.locator('[data-protected-routes-panel]')
  await panel.getByLabel('Name').fill(name)
  await panel.getByLabel('Public host').fill('browser.invalid')
  await panel.getByLabel('Public path').fill('/mcp')
  await panel.getByLabel('Loadout').click()
  await page.getByRole('option', { name: loadout, exact: true }).click()
  const mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && response.request().postData()?.includes('gateway.protected_route.stage_add') === true)
  await panel.getByRole('button', { name: 'Add route', exact: true }).last().click()
  const response = await mutation
  assert.equal(response.status(), 200, `Protected route staging returned ${response.status()}: ${await response.text()}`)
  await assert.doesNotReject(panel.getByText(name, { exact: true }).first().waitFor({ state: 'visible', timeout: 10_000 }))
  await assert.doesNotReject(panel.getByText(/saved for restart/i).waitFor({ state: 'visible', timeout: 10_000 }))
}

async function removeProtectedRouteThroughUi(page: Page, name: string) {
  const panel = page.locator('[data-protected-routes-panel]')
  const removeButton = panel.getByRole('button', { name: `Remove protected route ${name}` })
  await removeButton.click()
  // Removing a just-staged add cancels it outright; removing an already
  // mounted route instead leaves a restart-required tombstone.
  await assert.doesNotReject(removeButton.waitFor({ state: 'hidden', timeout: 10_000 }))
}

async function exerciseArtifactLibraryThroughUi(page: Page, csrfToken: string, name: string) {
  await page.goto('/skills/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  await page.getByRole('button', { name: 'Create skill', exact: true }).click()
  await page.getByLabel('Skill name').fill(name)
  await page.getByLabel('Contents').fill(`---\nname: ${name}\ndescription: Live browser Artifact lifecycle\n---\n\n# First revision\n`)
  await page.getByRole('button', { name: 'Validate', exact: true }).click()
  await assert.doesNotReject(page.getByText('Skill is valid').waitFor({ state: 'visible', timeout: 10_000 }))
  let mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.create') === true)
  await page.getByRole('button', { name: 'Save immutable revision', exact: true }).click()
  let response = await mutation
  assert.equal(response.status(), 200, `Artifact create returned ${response.status()}: ${await response.text()}`)
  await assert.doesNotReject(page.getByRole('button', { name: new RegExp(`^${name}`) }).waitFor({ state: 'visible', timeout: 10_000 }))

  await page.getByRole('button', { name: 'Activate latest revision', exact: true }).click()
  await assert.doesNotReject(page.getByText('Published', { exact: true }).last().waitFor({ state: 'visible', timeout: 10_000 }))

  await page.getByRole('button', { name: 'Edit latest', exact: true }).click()
  await page.getByLabel('Contents').fill(`---\nname: ${name}\ndescription: Live browser Artifact lifecycle\n---\n\n# Second revision\n`)
  mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.save') === true)
  await page.getByRole('button', { name: 'Save immutable revision', exact: true }).click()
  response = await mutation
  assert.equal(response.status(), 200, `Artifact save returned ${response.status()}: ${await response.text()}`)
  const afterSave = await action(page, csrfToken, 'artifacts', 'artifacts.list', { limit: 10 })
  progress(`artifact-after-save:${afterSave.status}:${JSON.stringify(afterSave.body).slice(0, 1000)}`)
  const activate = page.getByRole('button', { name: 'Activate latest revision', exact: true })
  await activate.waitFor({ state: 'visible', timeout: 10_000 })
  await activate.click({ timeout: 10_000 })

  page.once('dialog', dialog => dialog.accept())
  mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.archive') === true)
  await page.getByRole('button', { name: 'Archive', exact: true }).click()
  response = await mutation
  assert.equal(response.status(), 200, `Artifact archive returned ${response.status()}: ${await response.text()}`)
  const afterArchive = await action(page, csrfToken, 'artifacts', 'artifacts.list', { limit: 10 })
  assert.equal(afterArchive.status, 200)
  assert.equal(afterArchive.body.items.find((item: { name: string }) => item.name === name)?.archived, true)
}

async function exerciseActionableImportFailure(page: Page) {
  await page.goto('/skills/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  await page.getByRole('button', { name: 'Import', exact: true }).click()
  await page.getByLabel('Import connection').fill('unconfigured-repository')
  await page.getByLabel('Import artifact ID').fill('missing-artifact')
  await page.getByLabel('Import revision ID').fill(`sha256:${'a'.repeat(64)}`)
  const mutation = page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/v1/artifacts'
    && response.request().postData()?.includes('artifacts.import') === true)
  await page.getByRole('button', { name: 'Import exact revision', exact: true }).click()
  const response = await mutation
  assert.equal(response.status(), 503)
  const body = await response.json()
  assert.equal(body.kind, 'source_unavailable')
  await assert.doesNotReject(page.getByText(/import sources are not configured|source is not configured/i).waitFor({ state: 'visible', timeout: 10_000 }))
}

async function waitForFile(path: string, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (await access(path).then(() => true, () => false)) return
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error(`timed out waiting for ${path}`)
}

async function stashFileActionThroughUi(page: Page, name: string, action: 'Download' | 'Manage file' | 'Delete') {
  await page.getByRole('article').filter({ hasText: name }).getByRole('button', { name: `Actions for ${name}`, exact: true }).click()
  await page.getByRole('menuitem', { name: action === 'Download' ? `Download ${name}` : action, exact: true }).click()
  if (action === 'Download') {
    await page.keyboard.press('Escape')
    await page.getByRole('menu').waitFor({ state: 'hidden', timeout: 10_000 })
  }
}

async function exerciseStashThroughUi(page: Page, descriptor: Awaited<ReturnType<typeof readLiveDescriptor>>) {
  assert.ok(descriptor)
  let name = `browser-stash-${descriptor.run_id}.txt`
  const contents = `live browser stash ${descriptor.run_id}\n`
  // Finish the initial debounced list load before upload refreshes can overlap it.
  const initialList = page.waitForResponse(response => response.request().method() === 'GET'
    && new URL(response.url()).pathname === '/v1/stash')
  await page.goto('/stash/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  const initialResponse = await initialList
  assert.equal(initialResponse.status(), 200)
  assert.equal(await initialResponse.finished(), null)
  assert.deepEqual((await initialResponse.json()).files, [])
  await page.getByText('Your Stash is empty', { exact: true }).waitFor({ state: 'visible', timeout: 10_000 })
  await page.getByRole('status').filter({ hasText: 'Loading your files…' }).waitFor({ state: 'hidden', timeout: 10_000 })
  const dropTarget = page.getByRole('button', { name: /drop files here or browse/i })
  await dropTarget.waitFor({ state: 'visible', timeout: 10_000 })
  await dropTarget.focus()
  assert.equal(await dropTarget.evaluate(element => element === document.activeElement), true)
  const fileChooser = page.waitForEvent('filechooser')
  const uploadRequest = page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === '/v1/stash/uploads')
  await dropTarget.press('Enter')
  await (await fileChooser).setFiles({ name, mimeType: 'text/plain', buffer: Buffer.from(contents) })
  const observedUpload = await uploadRequest
  // File bodies are not inline CDP postData entries. A 201 means the server
  // validated the exact wire length; downloads below verify persisted bytes.
  assert.equal((await observedUpload.response())?.status(), 201, 'File Stash must accept the browser upload')
  await page.getByRole('status').filter({ hasText: `${name} uploaded.` }).waitFor({ state: 'attached', timeout: 15_000 })
  const row = page.getByRole('article').filter({ hasText: name })
  await row.waitFor({ state: 'visible', timeout: 10_000 })
  const uri = await row.getByTitle('Copy canonical URI').locator('code').innerText()
  assert.match(uri, /^stash:\/\/me\/files\/[A-Z0-9]+$/)
  await assert.doesNotReject(page.locator('[data-console-hero-stat="Files"] [data-console-hero-stat-value="1"][title="1"]').waitFor({ state: 'visible' }))
  await assert.doesNotReject(page.getByText(`${Buffer.byteLength(contents)} B`, { exact: true }).first().waitFor({ state: 'visible' }))

  const emptyName = `browser-stash-empty-${descriptor.run_id}.txt`
  const emptyRequest = page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === '/v1/stash/uploads')
  const emptyChooser = page.waitForEvent('filechooser')
  await dropTarget.press('Enter')
  await (await emptyChooser).setFiles({ name: emptyName, mimeType: 'text/plain', buffer: Buffer.alloc(0) })
  const observedEmptyUpload = await emptyRequest
  assert.equal((await observedEmptyUpload.response())?.status(), 201, 'File Stash must accept an empty browser upload')
  await page.getByRole('status').filter({ hasText: `${emptyName} uploaded.` }).waitFor({ state: 'attached', timeout: 15_000 })
  const emptyRow = page.getByRole('article').filter({ hasText: emptyName })
  // Upload completion precedes the list refresh, which replaces its action menus.
  await emptyRow.waitFor({ state: 'visible', timeout: 10_000 })
  await page.getByRole('status').filter({ hasText: 'Loading your files…' }).waitFor({ state: 'hidden', timeout: 10_000 })
  assert.equal(await page.getByRole('article').count(), 2)
  const [emptyDownload] = await Promise.all([
    page.waitForEvent('download'),
    stashFileActionThroughUi(page, emptyName, 'Download'),
  ])
  assert.equal(emptyDownload.suggestedFilename(), emptyName)
  assert.deepEqual(await readFile((await emptyDownload.path())!), Buffer.alloc(0), 'zero-byte upload must persist exactly')
  await stashFileActionThroughUi(page, emptyName, 'Delete')
  await page.getByRole('alertdialog', { name: 'Delete this file?' }).getByRole('button', { name: 'Delete file' }).click()
  await emptyRow.waitFor({ state: 'detached', timeout: 10_000 })

  const [download] = await Promise.all([
    page.waitForEvent('download'),
    stashFileActionThroughUi(page, name, 'Download'),
  ])
  assert.deepEqual(await readFile((await download.path())!), Buffer.from(contents), 'uploaded bytes must persist exactly')

  await stashFileActionThroughUi(page, name, 'Manage file')
  const dialog = page.getByRole('dialog', { name: `Manage ${name}` })
  await dialog.getByLabel('Filename').waitFor({ state: 'visible' })
  assert.equal(await dialog.getByLabel('Filename').evaluate(element => element === document.activeElement), true)
  const previousName = name
  name = `browser-stash-renamed-${descriptor.run_id}.txt`
  await dialog.getByLabel('Filename').fill(name)
  const renameResponse = page.waitForResponse(response => response.request().method() === 'PATCH'
    && new URL(response.url()).pathname.startsWith('/v1/stash/files/'))
  await dialog.getByRole('button', { name: 'Rename', exact: true }).click()
  assert.equal((await renameResponse).status(), 200)
  await row.waitFor({ state: 'detached', timeout: 10_000 })
  const renamed = page.getByRole('article').filter({ hasText: name })
  await renamed.waitFor({ state: 'visible', timeout: 10_000 })
  assert.equal(await renamed.getByTitle('Copy canonical URI').locator('code').innerText(), uri)
  await assert.doesNotReject(page.getByText(previousName, { exact: true }).waitFor({ state: 'detached', timeout: 10_000 }))

  await stashFileActionThroughUi(page, name, 'Manage file')
  const shareDialog = page.getByRole('dialog', { name: `Manage ${name}` })
  await shareDialog.getByLabel('Find a recipient').fill('Browser Stash')
  await shareDialog.getByText('Browser Stash Recipient', { exact: true }).waitFor({ state: 'visible', timeout: 10_000 })
  await shareDialog.getByRole('button', { name: 'Grant access' }).click()
  await page.getByRole('status').filter({ hasText: 'Access granted' }).waitFor({ state: 'attached', timeout: 10_000 })

  await writeFile(descriptor.restart_request_path, 'restart\n', { mode: 0o600, flag: 'wx' })
  await waitForFile(descriptor.restart_complete_path)
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
  const persisted = page.getByRole('article').filter({ hasText: name })
  await persisted.waitFor({ state: 'visible', timeout: 10_000 })

  await stashFileActionThroughUi(page, name, 'Manage file')
  const persistedDialog = page.getByRole('dialog', { name: `Manage ${name}` })
  const revoke = persistedDialog.getByRole('button', { name: 'Revoke' })
  await revoke.waitFor({ state: 'visible', timeout: 10_000 })
  await revoke.click()
  await page.getByRole('status').filter({ hasText: 'Access revoked' }).waitFor({ state: 'attached', timeout: 10_000 })

  const afterRevoke = page.getByRole('article').filter({ hasText: name })
  await stashFileActionThroughUi(page, name, 'Delete')
  const confirmation = page.getByRole('alertdialog', { name: 'Delete this file?' })
  await confirmation.getByRole('button', { name: 'Delete file' }).click()
  await page.getByRole('status').filter({ hasText: `${name} deleted.` }).waitFor({ state: 'attached', timeout: 10_000 })
  await assert.doesNotReject(afterRevoke.waitFor({ state: 'detached', timeout: 10_000 }))
}

test('embedded Gateway Admin completes a real backend journey', {
  concurrency: false,
  skip: liveEnabled ? false : 'outer supervisor did not supply LABBY_LIVE_BROWSER_DESCRIPTOR',
}, async () => {
  progress('test-start')
  const descriptor = await readLiveDescriptor()
  assert.ok(descriptor)
  let csrfToken = await readPrivateCsrf(descriptor)
  progress('descriptor-read')
  await withAbsoluteDeadline(async (signal) => {
    progress('chromium-launch-start')
    await useBrowserWithAbort(signal, async () => chromium.launch(await ownedBrowserLaunchOptions(chromium.executablePath())), async (browser) => {
    progress('chromium-launched')
    const context = await browser.newContext({
        baseURL: descriptor.base_url,
        storageState: descriptor.storage_state_path,
        viewport: { width: 1360, height: 900 },
        reducedMotion: 'reduce',
      })
    await context.tracing.start({ screenshots: false, snapshots: false, sources: false })
    const page = await context.newPage()
    // The outer supervisor owns the authenticated browser fixture and its
    // CSRF material. Attach that material at the transport boundary so UI
    // controls exercise their real action mapping without an out-of-band
    // /auth/session probe rotating the fixture token.
    let denyPaletteMutationCsrf = false
    let deniedPaletteAttempts = 0
    await page.route('**/v1/**', async (route) => {
      if (route.request().method() !== 'POST') return route.continue()
      const headers: Record<string, string> = { ...route.request().headers(), 'x-csrf-token': csrfToken }
      if (denyPaletteMutationCsrf
        && new URL(route.request().url()).pathname === '/v1/gateway'
        && route.request().postDataJSON()?.action === 'gateway.mcp.disable'
        && route.request().postDataJSON()?.params?.name === ownedName) {
        deniedPaletteAttempts += 1
        assert.ok(deniedPaletteAttempts <= 2, 'CSRF failure must not cause unbounded mutation retries')
        delete headers['x-csrf-token']
      }
      return route.continue({ headers })
    })
    const evidence = observeLivePage(page, descriptor.base_url)
    let failure: unknown
    const ownedName = `browser-${descriptor.run_id.toLowerCase()}`
    let ownedGatewayId = ownedName
    const ownedRoute = `${ownedName}-route`
    let ownedRouteRemoved = true
    let routeTeamId: string | undefined
    const ownedLoadout = `${ownedName}-loadout`
    let ownedLoadoutCreated = false
    const cleanupFailures: string[] = []
    try {
      progress('health-session-catalog')
      // Do not probe /auth/session out of band: that endpoint rotates CSRF
      // material, and the UI bootstrap must remain the sole browser-session
      // reader for this context.
      for (const route of ['/health', '/v1/catalog']) {
        const response = await page.request.get(route)
        progress(`${route}:${response.status()}`)
        assert.ok(response.ok(), `${route} returned ${response.status()}`)
      }
      const anonymous = await browser.newContext({ baseURL: descriptor.base_url })
      const denied = await anonymous.request.post('/v1/gateway', {
        data: { action: 'gateway.remove', params: { name: 'browser-denied' } },
      })
      await anonymous.close()
      assert.ok([401, 403].includes(denied.status()), `unauthorized mutation returned ${denied.status()}`)
      progress(`anonymous-denial:${denied.status()}`)

      await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
      progress('embedded-ui-loaded')
      // The palette loads its catalog only after an actual user opens it.
      // APIRequestContext probes above are not browser page-request evidence.
      const paletteTrigger = page.getByRole('button', { name: 'Search and filter', exact: true })
      const [paletteCatalog] = await Promise.all([
        page.waitForResponse(response => new URL(response.url()).origin === new URL(descriptor.base_url).origin
          && new URL(response.url()).pathname === '/v1/catalog'
          && response.request().method() === 'GET', { timeout: 15_000 }),
        paletteTrigger.click(),
      ])
      assert.equal(paletteCatalog.status(), 200)
      const paletteDialog = page.getByRole('dialog', { name: 'Command Palette', exact: true })
      await paletteDialog.waitFor({ state: 'visible' })
      const paletteSearch = paletteDialog.getByRole('textbox', { name: 'Search command palette', exact: true })
      await paletteSearch.waitFor({ state: 'visible' })
      assert.equal(await paletteSearch.evaluate(element => element === document.activeElement), true)
      await page.keyboard.press('Escape')
      await paletteDialog.waitFor({ state: 'hidden' })
      assert.equal(await paletteTrigger.evaluate(element => element === document.activeElement), true)
      progress('palette-catalog-through-ui')

      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
      progress(`page-errors:${evidence.pageErrors.length}:csp:${evidence.cspViolations.length}`)
      assert.equal(evidence.pageErrors.length, 0)
      assert.equal(evidence.cspViolations.length, 0)

      ownedGatewayId = await addGatewayThroughUi(page, ownedName)
      progress('gateway-added-through-ui')
      await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
      progress('page-reloaded')
      const persisted = await action(page, csrfToken, 'gateway', 'gateway.server.get', { id: ownedGatewayId })
      progress(`gateway-get:${persisted.status}`)
      assert.equal(persisted.status, 200)
      await assert.doesNotReject(page.locator('a:visible').filter({ hasText: ownedName }).first().waitFor({ state: 'visible', timeout: 10_000 }))

      // Exercise the real catalog wire `name` through the lazy schema, action
      // drill-down, parameter form, and adapter. The existing continuation
      // supplies CSRF; every response still comes from the owned Labby daemon.
      for (const width of [1360, 390]) {
        await page.setViewportSize({ width, height: 900 })
        await page.getByRole('button', { name: 'Search and filter', exact: true }).click()
        const palette = page.getByRole('dialog', { name: 'Command Palette', exact: true })
        await palette.getByRole('textbox', { name: 'Search command palette' }).fill('> gateway')
        await palette.getByRole('option').filter({ has: page.getByText('gateway', { exact: true }) }).click({ timeout: 10_000 })
        await palette.getByRole('textbox', { name: 'Search command palette' }).fill('gateway.server.get')
        await palette.getByRole('option').filter({ has: page.getByText('gateway.server.get', { exact: true }) }).click({ timeout: 10_000 })
        const serverId = palette.locator('#param-id')
        assert.equal(await serverId.getAttribute('required'), '', 'catalog must project the required server id')
        await serverId.fill(ownedGatewayId)
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
        const [paletteResponse] = await Promise.all([
          page.waitForResponse(response => response.request().method() === 'POST'
            && new URL(response.url()).pathname === '/v1/gateway'
            && response.request().postDataJSON()?.action === 'gateway.server.get', { timeout: 10_000 }),
          palette.getByRole('button', { name: 'Run', exact: true }).click(),
        ])
        assert.deepEqual(paletteResponse.request().postDataJSON(), {
          action: 'gateway.server.get', params: { id: ownedGatewayId },
        })
        assert.equal(paletteResponse.status(), 200, 'palette lookup must succeed against the real gateway')
        const paletteServer = await paletteResponse.json()
        assert.equal(paletteServer.id, ownedGatewayId)
        assert.equal(paletteServer.name, ownedName)
        assert.equal(paletteServer.source, 'custom_gateway')
        assert.equal(paletteServer.configured, true)
        await palette.waitFor({ state: 'hidden', timeout: 10_000 })
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
        progress(`catalog-palette-owned-server-get-through-ui:${width}`)
      }
      await page.setViewportSize({ width: 1360, height: 900 })

      // Reject an authenticated palette mutation at the real daemon's CSRF
      // boundary. Target only the owned gateway and explicitly disable both
      // optional process-cleanup paths for this reversible operation.
      const gatewayBefore = await action(page, csrfToken, 'gateway', 'gateway.get', { name: ownedName })
      assert.equal(gatewayBefore.status, 200)
      assert.equal(gatewayBefore.body.config.name, ownedName)
      assert.equal(gatewayBefore.body.config.enabled, true, 'owned gateway starts enabled')
      await page.getByRole('button', { name: 'Search and filter', exact: true }).click()
      const deniedPalette = page.getByRole('dialog', { name: 'Command Palette', exact: true })
      await deniedPalette.getByRole('textbox', { name: 'Search command palette' }).fill('> gateway')
      await deniedPalette.getByRole('option').filter({ has: page.getByText('gateway', { exact: true }) }).click({ timeout: 10_000 })
      await deniedPalette.getByRole('textbox', { name: 'Search command palette' }).fill('gateway.mcp.disable')
      await deniedPalette.getByRole('option').filter({ has: page.getByText('gateway.mcp.disable', { exact: true }) }).click({ timeout: 10_000 })
      await deniedPalette.locator('#param-name').fill(ownedName)
      await deniedPalette.locator('#param-cleanup').fill('false')
      await deniedPalette.locator('#param-aggressive').fill('false')
      const deniedResponses: import('playwright').Response[] = []
      let refreshedSessionResponse: import('playwright').Response | undefined
      const observeDeniedResponse = (response: import('playwright').Response) => {
        if (new URL(response.url()).pathname === '/auth/session') refreshedSessionResponse = response
        if (response.request().method() === 'POST'
          && new URL(response.url()).pathname === '/v1/gateway'
          && response.request().postDataJSON()?.action === 'gateway.mcp.disable') deniedResponses.push(response)
      }
      page.on('response', observeDeniedResponse)
      denyPaletteMutationCsrf = true
      try {
        await deniedPalette.getByRole('button', { name: 'Run', exact: true }).click()
        // validation_failed CSRF errors get one session refresh and one retry.
        // Strip the header on both attempts, and wait for the final UI failure.
        await page.getByText('gateway gateway.mcp.disable failed', { exact: true }).waitFor({ state: 'visible', timeout: 10_000 })
        assert.equal(deniedPaletteAttempts, 2)
        assert.equal(deniedResponses.length, 2)
        for (const response of deniedResponses) {
          assert.deepEqual(response.request().postDataJSON(), {
            action: 'gateway.mcp.disable', params: { name: ownedName, cleanup: false, aggressive: false },
          })
          assert.equal(response.status(), 422)
          const body = await response.json()
          assert.equal(body.kind, 'validation_failed')
          assert.match(body.message, /missing or invalid csrf token/i)
          await page.getByText(body.message, { exact: true }).waitFor({ state: 'visible', timeout: 10_000 })
        }
        assert.equal(await deniedPalette.isVisible(), true, 'denied action must keep its parameter form open')
      } finally {
        denyPaletteMutationCsrf = false
        page.off('response', observeDeniedResponse)
        if (refreshedSessionResponse?.ok()) {
          const refreshedSession = await refreshedSessionResponse.json()
          if (refreshedSession.authenticated && typeof refreshedSession.csrf_token === 'string' && refreshedSession.csrf_token.length > 0) {
            csrfToken = refreshedSession.csrf_token
          }
        }
      }
      // Reuse the client's real refresh response; another session probe would
      // rotate transport state again. Never log the private token.
      assert.ok(refreshedSessionResponse, 'CSRF retry must refresh the browser session')
      assert.equal(refreshedSessionResponse.status(), 200)
      await page.keyboard.press('Escape')
      await deniedPalette.waitFor({ state: 'hidden', timeout: 10_000 })
      const gatewayAfter = await action(page, csrfToken, 'gateway', 'gateway.get', { name: ownedName })
      assert.equal(gatewayAfter.status, 200)
      assert.equal(gatewayAfter.body.config.enabled, true, 'denied disable must leave the owned gateway enabled')
      assert.deepEqual(gatewayAfter.body.config, gatewayBefore.body.config, 'denied palette mutation must leave the complete owned gateway configuration unchanged')
      progress('catalog-palette-authenticated-csrf-denial-through-ui:422:unchanged')

      // Gateway policy belongs to an explicit Team. Use the authority projected
      // for this authenticated fixture, rather than the installation loadout.
      routeTeamId = persisted.sessionTeams?.find((team: { id?: unknown }) => typeof team.id === 'string' && team.id.length > 0)?.id
      assert.ok(routeTeamId, 'live browser owner must have a projected Team workspace')
      const loadout = await action(page, csrfToken, 'gateway', 'gateway.loadout.add', {
        loadout: { name: ownedLoadout, upstreams: [], services: [] },
      }, routeTeamId)
      assert.equal(loadout.status, 200, `owned Team loadout create failed: ${JSON.stringify(loadout.body)}`)
      ownedLoadoutCreated = true
      // Public Team projections strip the internal team:<id>: namespace.
      // The explicit workspace/header remains the authorization selector.
      assert.equal(loadout.body.name, ownedLoadout)
      await selectWorkspaceThroughUi(page, routeTeamId)
      ownedRouteRemoved = false
      await stageProtectedRouteThroughUi(page, ownedRoute, ownedLoadout)
      progress('protected-route-staged-through-ui')
      await removeProtectedRouteThroughUi(page, ownedRoute)
      ownedRouteRemoved = true
      progress('protected-route-removed-through-ui')
      await selectWorkspaceThroughUi(page)

      const libraryProbe = await action(page, csrfToken, 'artifacts', 'artifacts.list', { limit: 10 })
      progress(`artifact-library-probe:${libraryProbe.status}:project=${libraryProbe.sessionProjectId}:${JSON.stringify(libraryProbe.body).slice(0, 500)}`)
      assert.equal(libraryProbe.status, 200, `Artifact Library probe failed: ${JSON.stringify(libraryProbe.body)}`)
      await exerciseArtifactLibraryThroughUi(page, csrfToken, `${ownedName}-skill`)
      progress('artifact-lifecycle-through-ui')
      await exerciseActionableImportFailure(page)
      progress('artifact-import-failure-through-ui')
      await exerciseSnippetReplayThroughUi(page, csrfToken, `${ownedName}-replay`)

      if (descriptor.stash_supported) {
        await exerciseStashThroughUi(page, descriptor)
        progress('stash-lifecycle-reload-restart-through-ui')
      }

      // A real rapid duplicate reaches backend serialization; the UI must not
      // turn it into two successful state transitions.
      const duplicate = await Promise.all([
        action(page, csrfToken, 'gateway', 'gateway.add', { spec: { name: `${ownedName}-duplicate`, url: 'http://127.0.0.1:9/mcp' } }),
        action(page, csrfToken, 'gateway', 'gateway.add', { spec: { name: `${ownedName}-duplicate`, url: 'http://127.0.0.1:9/mcp' } }),
      ])
      assert.equal(duplicate.filter((result) => result.status === 200).length, 1)
      assert.ok(duplicate.some((result) => result.status === 409 || result.status >= 400))
      progress('duplicate-serialized')
      progress(`requests:count=${evidence.requests.length}:failures=${evidence.requests.filter((request) => (request.status ?? 0) >= 400).length}`)

      assert.ok(evidence.requests.some((request) => request.path === '/auth/session'))
      assert.ok(evidence.requests.some((request) => request.path === '/v1/catalog'))
      assert.ok(evidence.requests.some((request) => request.path === '/v1/gateway' && request.method === 'POST'))
      const scanSecrets = await readScanSecrets(descriptor, signal)
      assertCanaryFree(await page.locator('body').innerText(), scanSecrets, 'DOM')
      assertCanaryFree(evidence, scanSecrets, 'browser evidence')
      progress('evidence-asserted')
      await context.tracing.stop()
      progress('trace-stopped')
    } catch (error) {
      failure = error
      await captureFailureEvidence({ browser, context, page, descriptor, evidence, error, signal })
      throw error
    } finally {
      await runBrowserCleanupIfActive(signal, () => progress('cleanup-deferred-to-owned-root:deadline'), async (mayContinue) => {
      for (const [name, operation] of [
        ...(!ownedRouteRemoved ? [[ownedRoute, 'gateway.protected_route.stage_remove'] as const] : []),
        [`${ownedName}-duplicate`, 'gateway.remove'],
        [ownedGatewayId, 'gateway.remove'],
      ] as const) {
        if (!mayContinue()) break
        const result = await action(page, csrfToken, 'gateway', operation, { name, confirm: true }, operation === 'gateway.protected_route.stage_remove' ? routeTeamId : undefined).catch((error) => ({ status: 0, body: String(error) }))
        progress(`cleanup:${operation}:${result.status}`)
        if (![200, 404].includes(result.status) && operation === 'gateway.remove') {
          if (!mayContinue()) break
          const absent = await action(page, csrfToken, 'gateway', 'gateway.get', { name })
          progress(`cleanup-observe:${name}:${absent.status}`)
          if (absent.status === 404) continue
          if (result.status >= 500 && absent.status >= 500) {
            // The outer Rust supervisor owns and verifies deletion of the
            // complete disposable installation after this real 5xx path.
            progress(`cleanup-deferred-to-owned-root:${name}`)
            continue
          }
        }
        if (![200, 404].includes(result.status)) cleanupFailures.push(`${operation}(${name})=${result.status}`)
      }
      if (ownedLoadoutCreated && mayContinue()) {
        const result = await action(page, csrfToken, 'gateway', 'gateway.loadout.remove', { name: ownedLoadout }, routeTeamId)
          .catch((error) => ({ status: 0, body: String(error) }))
        progress(`cleanup:gateway.loadout.remove:${result.status}`)
        if (![200, 404].includes(result.status)) cleanupFailures.push(`gateway.loadout.remove(${ownedLoadout})=${result.status}`)
      }
      })
      if (!failure) {
        // `/auth/session` rotates the browser cookie. Preserve the current
        // authenticated state for the separately-created mobile context.
        await context.storageState({ path: descriptor.storage_state_path })
        await context.close()
      }
      else await context.close().catch(() => undefined)
      if (!failure) assert.deepEqual(cleanupFailures, [], `live browser cleanup failed: ${cleanupFailures.join(', ')}`)
    }
    })
  }, 'live Gateway Admin journey')
})

test('nightly mobile viewport has no overflow and essential landmarks', {
  concurrency: false,
  skip: liveEnabled && nightlyEnabled ? false : 'nightly live browser coverage is disabled',
}, async () => {
  const descriptor = await readLiveDescriptor()
  assert.ok(descriptor)
  await withAbsoluteDeadline(async (signal) => {
    await useBrowserWithAbort(signal, async () => chromium.launch(await ownedBrowserLaunchOptions(chromium.executablePath())), async (browser) => {
    const context = await browser.newContext({
      baseURL: descriptor.base_url, storageState: descriptor.storage_state_path,
      viewport: { width: 390, height: 844 },
    })
    const page = await context.newPage()
    await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
    await page.getByRole('main').waitFor({ state: 'visible', timeout: 10_000 })
    const navigationToggle = page.getByRole('button', { name: 'Open navigation' })
    await navigationToggle.waitFor({ state: 'visible', timeout: 10_000 })
    await navigationToggle.click()
    const navigationDialog = page.getByRole('dialog', { name: 'Navigation' })
    await navigationDialog.waitFor({ state: 'visible', timeout: 10_000 })
    await navigationDialog.getByRole('navigation').waitFor({ state: 'visible', timeout: 10_000 })
    await context.close()
    })
  }, 'nightly live Gateway Admin journey')
})
