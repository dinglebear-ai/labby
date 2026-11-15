import {describe, test, before, after} from 'node:test'
import assert from 'node:assert/strict'
import {execFileSync} from 'node:child_process'
import {readFileSync} from 'node:fs'
import {resolve, join} from 'node:path'
import {chromium, expect} from '@playwright/test'
import {teamInventoryRow, withInvitationClipboard} from './provider-access-checks.mjs'

// Offline structural/cleanup regressions. This fixture establishes no domain
// state, identity, session, invitation, or provider acceptance result.
describe('offline native Access markup and sensitive-output cleanup', {timeout: 60000}, () => {
  let browser
  let context
  let page
  let rendered
  let requests = 0
  const token = 'a1'.repeat(32)
  before(async () => {
    const env = {...process.env, MIX_ENV: 'test', ERL_FLAGS: '+S 4:4'}
    delete env.MIX_BUILD_PATH
    const application = resolve(import.meta.dirname, '..')
    const html = execFileSync('mix', ['run', '--no-start', '--no-compile',
      join(import.meta.dirname, 'access-offline-markup.exs')], {
      cwd: process.env.LABBY_E2E_ACCESS_RENDER_APP || application,
      env, encoding: 'utf8', timeout: 15000, maxBuffer: 262144,
    })
    rendered = JSON.parse(html)
    browser = await chromium.launch({headless: true,
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH || '/usr/bin/google-chrome',
      timeout: 10000})
    context = await browser.newContext({offline: true})
    context.on('request', () => requests++)
    page = await context.newPage()
    page.setDefaultTimeout(1000)
  })
  after(async () => {
    try {
      await context?.close()
    } finally {
      await browser?.close()
    }
    assert.equal(requests, 0, 'offline regressions must make no HTTP requests')
  })

  for (const status of ['suspended', 'active']) {
    test(`rendered owner-role Team is selected by its unique name while retaining exact ID (${status})`, async () => {
      await page.setContent(rendered.markup[status])
      await expect(page.locator('#access-team')).toHaveValue(rendered.team.id)
      await expect(teamInventoryRow(page, rendered.team)).toHaveCount(1)
      await expect(teamInventoryRow(page, rendered.team).locator('[data-status]')).toHaveAttribute('data-status', status)
    })
  }

  async function clipboardFixture() {
    await page.setContent(rendered.markup.active)
    const source = readFileSync(resolve(import.meta.dirname, '../assets/js/access_secrets.mjs'), 'utf8')
      .replace('export function newInvitationToken', 'function newInvitationToken')
      .replace('export default', 'const accessHook =')
    await page.addScriptTag({content: `${source}\nwindow.offlineAccessHook = accessHook`})
    await page.evaluate(value => {
      window.offlineClipboard = {value: '', clears: 0, rejectRead: false, rejectClear: false, hangRead: false}
      Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {
        readText: async () => {
          if (window.offlineClipboard.hangRead) return new Promise(() => {})
          if (window.offlineClipboard.rejectRead) throw Error('CONTROLLED_READ_FAILURE')
          return window.offlineClipboard.value
        },
        writeText: async text => {
          if (text === '' && window.offlineClipboard.rejectClear) throw Error('CONTROLLED_CLEAR_FAILURE')
          window.offlineClipboard.value = text
          if (text === '') window.offlineClipboard.clears++
        },
      }})
      const hook = {el: document.querySelector('#access-main'), handleEvent: () => {}}
      window.offlineAccessHook.mounted.call(hook)
      hook.ready = true
      hook.secret = value
      document.querySelector('#access-created-secret').value = value
      document.querySelector('[data-access-secret-output]').hidden = false
    }, token)
  }

  test('post-copy failure still clears owned token before the page/context can close', async () => {
    await clipboardFixture()
    const receipt = {}
    const primary = Error('CONTROLLED_POST_COPY_FAILURE')
    let observed
    try {
      await withInvitationClipboard(page, token, receipt, async () => {
        await page.locator('[data-access-copy]').click()
        await expect(page.locator('[data-access-copy-status]')).toContainText('Token copied')
        throw primary
      })
    } catch (error) { observed = error }
    assert.equal(observed === primary, true)
    assert.equal(page.isClosed(), false)
    assert.equal(await page.evaluate(() => window.offlineClipboard.value === ''), true)
    assert.equal(receipt.clipboardCleanup, 'cleared')
  })

  test('cleanup preserves unrelated clipboard replacement', async () => {
    await clipboardFixture()
    const receipt = {}
    await withInvitationClipboard(page, token, receipt, async () => {
      await page.locator('[data-access-copy]').click()
      await page.evaluate(() => { window.offlineClipboard.value = 'unrelated-owned-fixture-content' })
    })
    assert.equal(await page.evaluate(() => window.offlineClipboard.value === 'unrelated-owned-fixture-content'), true)
    assert.equal(await page.evaluate(() => window.offlineClipboard.clears), 0)
    assert.equal(receipt.clipboardCleanup, 'owned-content-absent')
  })

  for (const mode of ['rejectRead', 'rejectClear']) {
    test(`unconfirmed ${mode} cleanup fails without replacing a primary failure`, async () => {
      await clipboardFixture()
      const receipt = {}
      const primary = Error('CONTROLLED_POST_COPY_FAILURE')
      let observed
      try {
        await withInvitationClipboard(page, token, receipt, async () => {
          await page.locator('[data-access-copy]').click()
          await page.evaluate(key => { window.offlineClipboard[key] = true }, mode)
          throw primary
        })
      } catch (error) { observed = error }
      assert.equal(observed === primary, true)
      assert.equal(receipt.clipboardCleanup, 'unconfirmed')
      // Remove only this controlled fixture's content before reusing the page.
      await page.evaluate(() => {
        window.offlineClipboard.rejectRead = false
        window.offlineClipboard.rejectClear = false
        window.offlineClipboard.value = ''
      })
    })
  }

  test('unresponsive clipboard cleanup has a bounded deadline', async () => {
    await clipboardFixture()
    const receipt = {}
    const started = Date.now()
    let code
    try {
      await withInvitationClipboard(page, token, receipt, async () => {
        await page.locator('[data-access-copy]').click()
        await page.evaluate(() => { window.offlineClipboard.hangRead = true })
      })
    } catch (error) { code = error.message }
    assert.equal(code, 'ACCESS_CLIPBOARD_CLEANUP_UNCONFIRMED')
    assert.equal(receipt.clipboardCleanup, 'unconfirmed')
    assert.equal(Date.now() - started < 3000, true)
    await page.evaluate(() => { window.offlineClipboard.value = '' })
  })

  test('unconfirmed cleanup fails an otherwise successful exercise', async () => {
    await clipboardFixture()
    const receipt = {}
    let code
    try {
      await withInvitationClipboard(page, token, receipt, async () => {
        await page.locator('[data-access-copy]').click()
        await page.evaluate(() => { window.offlineClipboard.rejectRead = true })
      })
    } catch (error) { code = error.message }
    assert.equal(code, 'ACCESS_CLIPBOARD_CLEANUP_UNCONFIRMED')
    assert.equal(receipt.clipboardCleanup, 'unconfirmed')
    await page.evaluate(() => { window.offlineClipboard.value = '' })
  })
})
