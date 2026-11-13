import {test, expect} from '@playwright/test'
import {signIn, readyCatalog, gatewayAction} from './mcp-support.mjs'

// Run serially in the owning real harness. This journey uses its isolated
// configured peers and has a two-minute deadline and fewer than 100 requests.
test('native Gateway fleet preserves real observations, preferences and read navigation', async ({page}, info) => {
  test.setTimeout(120000)
  await readyCatalog()
  const inventory = await gatewayAction('gateway.list')
  const fixture = inventory.find(row => row.name === 'real-e2e-disabled')
  expect(fixture).toBeTruthy()
  expect(fixture.enabled).toBe(false)
  await signIn(page)
  await expect(page.locator('#servers-refresh')).toBeEnabled()
  await expect(page.getByRole('link', {name: 'Real E2E Disabled', exact: true})).toBeVisible()
  await page.getByLabel('Search servers', {exact: true}).fill('real-e2e-disabled')
  await expect(page.locator('#server-list li')).toHaveCount(1)
  await expect(page.locator('#server-list [data-state=disabled]')).toBeVisible()
  await page.getByRole('button', {name: 'Quick inspect Real E2E Disabled', exact: true}).click()
  await expect(page.locator('#gateway-inspector')).toContainText('Real E2E Disabled')
  const tools = fixture.capability_observation?.tools
  const measured = Number.isInteger(tools?.exposed) && Number.isInteger(tools?.discovered)
  await expect(page.locator('#gateway-inspector')).toContainText(measured ? `${tools.exposed} / ${tools.discovered}` : 'Unknown')
  await page.keyboard.press('Escape')
  await expect(page.locator('#gateway-inspector')).toHaveCount(0)
  await expect(page.getByRole('button', {name: 'Quick inspect Real E2E Disabled', exact: true})).toBeFocused()
  await page.getByRole('button', {name: 'Quick inspect Real E2E Disabled', exact: true}).click()
  await page.getByRole('button', {name: 'Close server inspector', exact: true}).click()
  await expect(page.locator('#gateway-inspector')).toHaveCount(0)
  await expect(page.getByRole('button', {name: 'Quick inspect Real E2E Disabled', exact: true})).toBeFocused()
  // Refresh removes the old row DOM, then reconciles the same selected ID.
  // Each remounted inspector must save the newly connected row button.
  for (const closeWith of ['escape', 'close']) {
    await page.getByRole('button', {name: 'Quick inspect Real E2E Disabled', exact: true}).click()
    await expect(page.locator('#gateway-inspector')).toBeVisible()
    await page.locator('#servers-refresh').click()
    await expect(page.locator('#servers-refresh')).toBeEnabled()
    await expect(page.locator('#gateway-inspector-title')).toBeFocused()
    if (closeWith === 'escape') await page.keyboard.press('Escape')
    else await page.getByRole('button', {name: 'Close server inspector', exact: true}).click()
    await expect(page.locator('#gateway-inspector')).toHaveCount(0)
    await expect(page.getByRole('button', {name: 'Quick inspect Real E2E Disabled', exact: true})).toBeFocused()
  }
  await chooseViewAction(page, 'Cards')
  await expect(page.locator('#gateway-inventory')).toHaveAttribute('data-layout', 'cards')
  await page.setViewportSize({width: 1440, height: 900})
  await chooseViewAction(page, 'Table')
  await expect(page.locator('#gateway-inventory')).toHaveAttribute('data-layout', 'table', {timeout: 5000})
  await chooseViewAction(page, 'Move skills earlier')
  await expect.poll(async () => {
    const columns = await page.locator('#server-list li').first().locator('[data-column]').evaluateAll(cells => cells.map(cell => cell.dataset.column))
    const skills = columns.indexOf('skills')
    const prompts = columns.indexOf('prompts')
    return skills >= 0 && prompts >= 0 && skills < prompts
  }, {timeout: 5000}).toBe(true)
  await chooseViewAction(page, 'Resources')
  await expect(page.locator('#server-list [data-column=resources]')).toHaveCount(0)
  for (const column of ['tools', 'prompts', 'skills', 'source']) {
    const snapshot = await page.locator(`#server-list [data-column=${column}]`).ariaSnapshot()
    expect(snapshot).toContain(column[0].toUpperCase() + column.slice(1))
  }
  await chooseViewAction(page, 'Cards')
  const key = await page.locator('#gateway-preferences').getAttribute('data-preference-key')
  await expect.poll(() => page.evaluate(key => JSON.parse(localStorage.getItem(`labby:gateway:${key}:v1`) || '{}').layout, key)).toBe('cards')
  await page.reload()
  await expect(page.locator('#gateway-inventory')).toHaveAttribute('data-layout', 'cards')
  for (const width of [1440, 740, 320]) {
    await page.setViewportSize({width, height: 900})
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await page.screenshot({path: info.outputPath(`gateway-fleet-${width}.png`), fullPage: true})
  }
  await page.getByRole('button', {name: 'Quick inspect Real E2E Disabled', exact: true}).click()
  await page.getByRole('link', {name: 'Catalog', exact: true}).click()
  await expect(page).toHaveURL(/\/gateway\/servers\/real-e2e-disabled\/catalog$/)
  await page.goto('/gateway')
  await page.locator('#servers-refresh').click()
  await expect(page.locator('#servers-refresh')).toBeEnabled()
  await expect(page.locator('#servers-error')).toHaveCount(0)
})


async function chooseViewAction(page, name) {
  const menu = page.locator('#gateway-view-menu')
  if (await menu.getAttribute('open') === null) await menu.locator('summary').click()
  const action = menu.getByRole('button', {name, exact: true})
  await expect(action, `View menu action ${name} should be visible`).toBeVisible({timeout: 5000})
  await action.click()
}
