import {test, expect} from '@playwright/test'
import {randomBytes} from 'node:crypto'
import {signIn, gatewayAction} from './mcp-support.mjs'
import {runPrivateScenario} from './private-scenario.mjs'

// The parent runs this file only after both source reviews. It uses the owned
// real Rust daemon and real MCP peer. Each serial scenario reserves four of its
// 30 Gateway action slots for cleanup, and never retries a submitted write.
test.describe.configure({mode: 'serial'})
test.use({screenshot: 'off', trace: 'off', video: 'off'})

function scenarioBudget() {
  const receipt = {gatewayEstimated: 0, gatewayActionLimit: 30}
  return {receipt, charge(count, cleanup = false) {
    receipt.gatewayEstimated += count
    if (receipt.gatewayEstimated > (cleanup ? 30 : 26)) throw new Error('Native Gateway scenario exceeded its action budget')
  }}
}

async function settled(page, step, prefix) {
  await step(`${prefix}-add-ready`, () => expect(page.locator('#gateway-add')).toBeEnabled({timeout: 35000}))
  await step(`${prefix}-refresh-ready`, () => expect(page.locator('#servers-refresh')).toBeEnabled())
  await step(`${prefix}-receipt-cleared`, () => expect(page.locator('#gm-receipt')).toHaveCount(0))
}

async function edit(page, name, charge, step, prefix) {
  charge(1) // fresh gateway.get
  await step(`${prefix}-edit-open`, () => page.getByRole('button', {name: `Edit ${name}`, exact: true}).first().click())
  await step(`${prefix}-edit-loaded`, () => expect(page.locator('#gm-name')).toHaveValue(name))
}

async function save(page, charge, step, prefix, rename = false) {
  charge(rename ? 4 : 3) // update, affected get(s), fleet list
  await step(`${prefix}-save-submit-once`, () => page.locator('#gm-save').click())
  await settled(page, step, prefix)
}

async function remove(page, name, charge, step) {
  charge(1) // fresh gateway.get before confirmation
  await step('remove-open', () => page.getByRole('button', {name: `Delete ${name}`, exact: true}).first().click())
  await step('remove-form-ready', () => expect(page.locator('#gm-remove-form')).toBeVisible())
  await step('remove-wrong-confirmation', () => page.getByLabel('Type the exact server ID to confirm').fill('wrong-id'))
  await step('remove-reject-submit', () => page.locator('#gm-remove').click())
  await step('remove-rejection', () => expect(page.locator('#gm-errors')).toContainText('exact selected server ID'))
  await step('remove-exact-confirmation', () => page.getByLabel('Type the exact server ID to confirm').fill(name))
  charge(3) // remove, affected get, fleet list
  await step('remove-submit-once', () => page.locator('#gm-remove').click())
  await settled(page, step, 'remove')
}

async function cleanup(page, ownedNames, charge) {
  // The scenario runner preserves the primary safe stage if this bounded cleanup
  // fails. Only generated names in a fresh authoritative inventory are removed.
  if (await page.locator('#gm-close').count()) await page.locator('#gm-close').click({timeout: 3000})
  charge(1, true)
  const rows = await gatewayAction('gateway.list')
  for (const name of ownedNames) {
    if (rows.some(row => row.name === name && row.source === 'custom_gateway')) {
      charge(1, true)
      await gatewayAction('gateway.remove', {name})
    }
  }
  charge(1, true)
  const after = await gatewayAction('gateway.list')
  expect(after.some(row => ownedNames.has(row.name))).toBe(false)
}

test('native local JSON, ephemeral draft probe, add, rename and persisted removal use real Gateway state', async ({page}, info) => {
  test.setTimeout(180000)
  const {charge, receipt} = scenarioBudget()
  const name = `native-crud-${randomBytes(5).toString('hex')}`
  const renamed = `${name}-renamed`
  const owned = new Set([name, renamed])
  const privateSentinel = `native-private-${randomBytes(8).toString('hex')}`
  const sentinel = 'real-e2e-disabled'
  await runPrivateScenario({page, info, accounting: receipt, cleanup: () => cleanup(page, owned, charge)}, async step => {
    charge(1)
    const before = await step('initial-inventory-read', () => gatewayAction('gateway.list'))
    const untouched = before.find(row => row.name === sentinel)
    await step('sentinel-present', async () => expect(untouched).toBeTruthy())
    charge(1)
    const sentinelConfig = (await step('sentinel-config-read', () => gatewayAction('gateway.get', {name: sentinel}))).config
    charge(1) // initial native fleet list
    await step('sign-in-connected', () => signIn(page))
    await settled(page, step, 'initial')
    await step('add-open', () => page.locator('#gateway-add').click())
    await step('json-open', () => page.locator('#gm-json').click())
    await step('json-private-input', () => page.getByLabel('JSON configuration').fill(JSON.stringify({mcpServers: {[name]: {url: 'http://127.0.0.1:4300/mcp', enabled: true, args: [], env: {NATIVE_PRIVATE: privateSentinel}, headers: {'X-Native-Private': privateSentinel}}}})))
    await step('json-apply', () => page.locator('#gm-json-apply').click())
    await step('json-name-loaded', () => expect(page.locator('#gm-name')).toHaveValue(name))
    charge(1)
    await step('json-no-save-read', async () => expect((await gatewayAction('gateway.list')).map(row => row.name).sort()).toEqual(before.map(row => row.name).sort()))
    charge(1) // draft gateway.test, with no persistent add
    await step('draft-test-submit-once', () => page.locator('#gm-test').click())
    await step('draft-test-connected', () => expect(page.locator('#gm-test-result')).toContainText('Probe connected', {timeout: 35000}))
    charge(1)
    await step('draft-test-no-save-read', async () => expect((await gatewayAction('gateway.list')).map(row => row.name).sort()).toEqual(before.map(row => row.name).sort()))
    await save(page, charge, step, 'add')
    await step('add-accepted', () => expect(page.locator('#gm-notice')).toContainText('accepted'))
    charge(1)
    const saved = await step('add-config-read', () => gatewayAction('gateway.get', {name}))
    await step('add-config-url', async () => expect(saved.config.url).toBe('http://127.0.0.1:4300/mcp'))
    await step('add-config-redacted', async () => expect(JSON.stringify(saved).includes(privateSentinel)).toBe(false))
    await step('add-dom-redacted', async () => expect((await page.content()).includes(privateSentinel)).toBe(false))
    await edit(page, name, charge, step, 'rename')
    await step('rename-input', () => page.locator('#gm-name').fill(renamed))
    await save(page, charge, step, 'rename', true)
    charge(1)
    const updated = await step('rename-config-read', () => gatewayAction('gateway.get', {name: renamed}))
    await step('rename-config-name', async () => expect(updated.config.name).toBe(renamed))
    await step('rename-config-url', async () => expect(updated.config.url).toBe('http://127.0.0.1:4300/mcp'))
    charge(1)
    await step('persisted-reload', () => page.reload())
    await settled(page, step, 'persisted')
    await step('persisted-renamed-row', () => expect(page.getByRole('button', {name: `Edit ${renamed}`, exact: true})).toBeVisible())
    for (const width of [1440, 740, 320]) {
      const prefix = `layout-${width}`
      await step(`${prefix}-viewport`, () => page.setViewportSize({width, height: 900}))
      await step(`${prefix}-add-open`, () => page.locator('#gateway-add').click())
      await step(`${prefix}-dialog`, () => expect(page.locator('#gm-dialog')).toBeVisible())
      await step(`${prefix}-overflow`, () => expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true))
      await step(`${prefix}-tab`, () => page.keyboard.press('Tab'))
      await step(`${prefix}-focus`, () => expect(page.locator('#gm-dialog :focus')).toHaveCount(1))
      // This is a new empty form, after the private draft and its DOM redaction check.
      await step(`${prefix}-screenshot`, () => page.screenshot({path: info.outputPath(`gateway-form-${width}.png`), fullPage: true}))
      await step(`${prefix}-escape`, () => page.keyboard.press('Escape'))
      await step(`${prefix}-focus-return`, () => expect(page.locator('#gateway-add')).toBeFocused())
    }
    await remove(page, renamed, charge, step)
    charge(1)
    const after = await step('removed-inventory-read', () => gatewayAction('gateway.list'))
    await step('removed-generated-names', async () => expect(after.some(row => owned.has(row.name))).toBe(false))
    await step('sentinel-enabled-unchanged', async () => expect(after.find(row => row.name === sentinel)?.enabled).toBe(untouched.enabled))
    charge(1)
    await step('sentinel-config-unchanged', async () => expect((await gatewayAction('gateway.get', {name: sentinel})).config).toEqual(sentinelConfig))
  })
})

test('native explicit disable and enable persist through independent reads and page reload', async ({page}, info) => {
  test.setTimeout(180000)
  const {charge, receipt} = scenarioBudget()
  const name = `native-toggle-${randomBytes(5).toString('hex')}`
  await runPrivateScenario({page, info, accounting: receipt, cleanup: () => cleanup(page, new Set([name]), charge)}, async step => {
    charge(1)
    await step('sign-in-connected', () => signIn(page))
    await settled(page, step, 'initial')
    await step('add-open', () => page.locator('#gateway-add').click())
    await step('add-name', () => page.getByLabel('ID', {exact: true}).fill(name))
    await step('add-endpoint', () => page.getByLabel('Endpoint URL', {exact: true}).fill('http://127.0.0.1:4300/mcp'))
    await save(page, charge, step, 'add')
    for (const enabled of [false, true]) {
      const prefix = enabled ? 'enable' : 'disable'
      await edit(page, name, charge, step, prefix)
      await step(`${prefix}-input`, () => page.getByLabel('Enable server', {exact: true}).setChecked(enabled))
      await save(page, charge, step, prefix)
      charge(1)
      await step(`${prefix}-config-read`, async () => expect((await gatewayAction('gateway.get', {name})).config.enabled).toBe(enabled))
    }
    charge(1)
    await step('persisted-reload', () => page.reload())
    await settled(page, step, 'persisted')
    await step('persisted-row', () => expect(page.getByRole('button', {name: `Edit ${name}`, exact: true})).toBeVisible())
    await remove(page, name, charge, step)
  })
})
