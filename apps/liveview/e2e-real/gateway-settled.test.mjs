import {test} from 'node:test'
import assert from 'node:assert/strict'
import {expect} from '@playwright/test'
import {gatewaySettled} from './mcp-support.mjs'
import {withGatewayFixture, gatewayFrameGate} from './gateway-fixture.mjs'

// Use the actual Gateway entrypoint and its existing mutation fixture. Delay one
// outbound LiveView event, preserving the normal click and backend lifecycle.
test('Gateway completion waits for each submitted save and removal before read-back', {timeout: 60000}, async context => {
  await withGatewayFixture({signal: context.signal}, async ({browser, port}) => {
    const page = await browser.newPage({baseURL: `http://127.0.0.1:${port}`})
    page.setDefaultTimeout(10000)
    assert.equal((await page.request.get('/assets/js/app.js', {timeout: 10000})).status(), 200)
    assert.equal((await page.request.get('/assets/css/workspace.css', {timeout: 10000})).status(), 200)
    const step = async (_, operation) => operation()
    let holdEvent
    let held
    let release
    await page.routeWebSocket('**/live/websocket*', socket => {
      const server = socket.connectToServer()
      socket.onMessage(message => {
        const payload = JSON.parse(message)[4]
        if (holdEvent && payload?.event === holdEvent) {
          release = () => server.send(message)
          held()
        } else server.send(message)
      })
    })
    await page.goto('/gateway')
    await page.getByLabel('Gateway credential').fill('fixture-only')
    await page.getByRole('button', {name: 'Sign in', exact: true}).click()
    await page.locator('[data-phx-session].phx-connected').waitFor()
    await gatewaySettled(page, step, 'initial')
    await page.locator('#gateway-add').click()
    await page.getByLabel('ID', {exact: true}).fill('helper-owned')
    await page.getByLabel('Endpoint URL', {exact: true}).fill('http://127.0.0.1:1/mcp')

    for (const [event, button] of [['gateway_save', '#gm-save'], ['gateway_confirm_remove', '#gm-remove']]) {
      if (event === 'gateway_confirm_remove') {
        await page.getByRole('button', {name: 'Delete helper-owned', exact: true}).first().click()
        await expect(page.locator('#gm-remove-form')).toBeVisible()
        await page.getByLabel('Type the exact server ID to confirm').fill('helper-owned')
      }
      holdEvent = event
      const frame = gatewayFrameGate(context.signal)
      held = frame.accept
      await Promise.all([page.locator(button).click(), frame.wait])
      let completed = false
      const settling = gatewaySettled(page, step, 'submitted').then(() => {completed = true})
      // Give the predicate browser round trips while its event is still held.
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))))
      try {
        assert.equal(completed, false, `${event} must not settle before its submitted event reaches LiveView`)
      } finally {
        holdEvent = undefined
        release()
        await settling
      }
      await expect(page.locator('#gm-dialog')).toHaveCount(0)
      await expect(page.getByRole('button', {name: 'Edit helper-owned', exact: true})).toHaveCount(event === 'gateway_save' ? 1 : 0)
    }
  })
})
