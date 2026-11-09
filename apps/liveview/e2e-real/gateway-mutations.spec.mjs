import {test, expect} from '@playwright/test'
import {randomBytes} from 'node:crypto'
import {signIn, gatewayAction} from './mcp-support.mjs'

// The parent runs this file only after both source reviews. It uses the owned
// real Rust daemon and real MCP peer. Each serial scenario reserves four of its
// 30 Gateway action slots for cleanup, and never retries a submitted write.
test.describe.configure({mode: 'serial'})

function scenarioBudget() {
  let used = 0
  return count => {
    used += count
    if (used > 30) throw new Error('Native Gateway scenario exceeded its 30-action budget')
  }
}

async function settled(page) {
  await expect(page.locator('#gateway-add')).toBeEnabled({timeout: 35000})
  await expect(page.locator('#servers-refresh')).toBeEnabled()
  await expect(page.locator('#gm-receipt')).toHaveCount(0)
}

async function edit(page, name, charge) {
  charge(1) // fresh gateway.get
  await page.getByRole('button', {name: `Edit ${name}`, exact: true}).first().click()
  await expect(page.locator('#gm-name')).toHaveValue(name)
}

async function save(page, charge, rename = false) {
  charge(rename ? 4 : 3) // update, affected get(s), fleet list
  await page.locator('#gm-save').click()
  await settled(page)
}

async function remove(page, name, charge) {
  charge(1) // fresh gateway.get before confirmation
  await page.getByRole('button', {name: `Delete ${name}`, exact: true}).first().click()
  await expect(page.locator('#gm-remove-form')).toBeVisible()
  await page.getByLabel('Type the exact server ID to confirm').fill('wrong-id')
  await page.locator('#gm-remove').click()
  await expect(page.locator('#gm-errors')).toContainText('exact selected server ID')
  await page.getByLabel('Type the exact server ID to confirm').fill(name)
  charge(3) // remove, affected get, fleet list
  await page.locator('#gm-remove').click()
  await settled(page)
}

async function cleanup(page, ownedNames, charge) {
  // Clear any private draft before the runner's failure screenshot. Cleanup is
  // restricted to the generated names after an authoritative current inventory.
  if (await page.locator('#gm-close').count()) await page.locator('#gm-close').click()
  charge(1)
  const rows = await gatewayAction('gateway.list')
  for (const name of ownedNames) {
    if (rows.some(row => row.name === name && row.source === 'custom_gateway')) {
      charge(1)
      await gatewayAction('gateway.remove', {name})
    }
  }
}

test('native local JSON, ephemeral draft probe, add, rename and persisted removal use real Gateway state', async ({page}, info) => {
  test.setTimeout(180000)
  const charge = scenarioBudget()
  const name = `native-crud-${randomBytes(5).toString('hex')}`
  const renamed = `${name}-renamed`
  const owned = new Set([name, renamed])
  const privateSentinel = `native-private-${randomBytes(8).toString("hex")}`
  const sentinel = 'real-e2e-disabled'
  charge(1)
  const before = await gatewayAction('gateway.list')
  const untouched = before.find(row => row.name === sentinel)
  expect(untouched).toBeTruthy()
  charge(1)
  const sentinelConfig = (await gatewayAction('gateway.get', {name: sentinel})).config
  try {
    charge(1) // initial native fleet list
    await signIn(page)
    await settled(page)
    await page.locator('#gateway-add').click()
    await page.locator('#gm-json').click()
    await page.getByLabel('JSON configuration').fill(JSON.stringify({mcpServers: {[name]: {url: 'http://127.0.0.1:4300/mcp', enabled: true, args: [], env: {NATIVE_PRIVATE: privateSentinel}, headers: {'X-Native-Private': privateSentinel}}}}))
    await page.locator('#gm-json-apply').click()
    await expect(page.locator('#gm-name')).toHaveValue(name)
    charge(1)
    expect((await gatewayAction('gateway.list')).map(row => row.name).sort()).toEqual(before.map(row => row.name).sort())
    charge(1) // draft gateway.test, with no persistent add
    await page.locator('#gm-test').click()
    await expect(page.locator('#gm-test-result')).toContainText('Probe connected', {timeout: 35000})
    charge(1)
    expect((await gatewayAction('gateway.list')).map(row => row.name).sort()).toEqual(before.map(row => row.name).sort())
    await save(page, charge)
    await expect(page.locator('#gm-notice')).toContainText('accepted')
    charge(1)
    const saved = await gatewayAction('gateway.get', {name})
    expect(saved.config.url).toBe('http://127.0.0.1:4300/mcp')
    expect(JSON.stringify(saved).includes(privateSentinel)).toBe(false)
    expect((await page.content()).includes(privateSentinel)).toBe(false)
    await edit(page, name, charge)
    await page.locator('#gm-name').fill(renamed)
    await save(page, charge, true)
    charge(1)
    const updated = await gatewayAction('gateway.get', {name: renamed})
    expect(updated.config.name).toBe(renamed)
    expect(updated.config.url).toBe('http://127.0.0.1:4300/mcp')
    charge(1)
    await page.reload()
    await settled(page)
    await expect(page.getByRole('button', {name: `Edit ${renamed}`, exact: true})).toBeVisible()
    for (const width of [1440, 740, 320]) {
      await page.setViewportSize({width, height: 900})
      await page.locator('#gateway-add').click()
      await expect(page.locator('#gm-dialog')).toBeVisible()
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      await page.keyboard.press('Tab')
      await expect(page.locator('#gm-dialog :focus')).toHaveCount(1)
      await page.screenshot({path: info.outputPath(`gateway-form-${width}.png`), fullPage: true})
      await page.keyboard.press('Escape')
      await expect(page.locator('#gateway-add')).toBeFocused()
    }
    await remove(page, renamed, charge)
    charge(1)
    const after = await gatewayAction('gateway.list')
    expect(after.some(row => owned.has(row.name))).toBe(false)
    expect(after.find(row => row.name === sentinel)?.enabled).toBe(untouched.enabled)
    charge(1)
    expect((await gatewayAction('gateway.get', {name: sentinel})).config).toEqual(sentinelConfig)
  } finally {
    await cleanup(page, owned, charge)
  }
})

test('native explicit disable and enable persist through independent reads and page reload', async ({page}) => {
  test.setTimeout(180000)
  const charge = scenarioBudget()
  const name = `native-toggle-${randomBytes(5).toString('hex')}`
  try {
    charge(1)
    await signIn(page)
    await settled(page)
    await page.locator('#gateway-add').click()
    await page.getByLabel('ID', {exact: true}).fill(name)
    await page.getByLabel('Endpoint URL', {exact: true}).fill('http://127.0.0.1:4300/mcp')
    await save(page, charge)
    for (const enabled of [false, true]) {
      await edit(page, name, charge)
      await page.getByLabel('Enable server', {exact: true}).setChecked(enabled)
      await save(page, charge)
      charge(1)
      expect((await gatewayAction('gateway.get', {name})).config.enabled).toBe(enabled)
    }
    charge(1)
    await page.reload()
    await settled(page)
    await expect(page.getByRole('button', {name: `Edit ${name}`, exact: true})).toBeVisible()
    await remove(page, name, charge)
  } finally {
    await cleanup(page, new Set([name]), charge)
  }
})
