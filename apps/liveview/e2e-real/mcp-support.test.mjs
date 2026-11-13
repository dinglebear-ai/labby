import {test} from 'node:test'
import assert from 'node:assert/strict'
import {chromium} from '@playwright/test'
import {signIn} from './mcp-support.mjs'

// Exercise the actual installed browser without reserving a console or Gateway
// port. Only the transport response is replaced, leaving signIn and its waits real.
test('signIn waits for the protected LiveView connection after its URL is ready', {timeout: 20000}, async () => {
  const browser = await chromium.launch({executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH})
  try {
    const page = await browser.newPage({baseURL: 'http://127.0.0.1:1'})
    await page.route('**/gateway', route => route.fulfill({contentType: 'text/html', body: '<main data-phx-session="synthetic-session">Gateway</main>'}))
    let completed = false
    const signingIn = signIn(page).then(() => { completed = true })
    await page.locator('[data-phx-session]').waitFor()
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))
    assert.equal(completed, false, 'a ready URL must not expose a disconnected LiveView to the caller')
    await page.locator('[data-phx-session]').evaluate(root => root.classList.add('phx-connected'))
    await signingIn
    assert.equal(completed, true)
  } finally {
    await browser.close()
  }
})

test('a real private-input timeout keeps its safe primary stage through cleanup and attachment failure', {timeout: 20000}, async () => {
  const {runPrivateScenario} = await import('./private-scenario.mjs')
  const browser = await chromium.launch({executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH})
  try {
    const page = await browser.newPage()
    await page.setContent('<input disabled aria-label="JSON configuration">')
    const privateValue = 'synthetic-private-input-do-not-persist'
    let cleanupCalls = 0
    let receipt
    await assert.rejects(runPrivateScenario({page,
      info: {attach: async (_, value) => {receipt = value.body; throw Error(privateValue)}},
      accounting: {gatewayEstimated: 1},
      cleanup: async () => {cleanupCalls++; throw Error(privateValue)},
    }, async step => {
      await step('json-private-input', () => page.getByLabel('JSON configuration').fill(privateValue, {timeout: 50}))
    }), error => {
      assert.equal(error.message, 'PRIVATE_SCENARIO_FAILED stage=json-private-input kind=timeout')
      assert.equal(error.cause, undefined)
      assert.equal(error.stack.includes(privateValue), false)
      return true
    })
    assert.equal(cleanupCalls, 1)
    assert.equal(JSON.parse(receipt).cleanup, 'unconfirmed')
    assert.equal(receipt.includes(privateValue), false)
  } finally {
    await browser.close()
  }
})
