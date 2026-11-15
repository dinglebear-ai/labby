import {isGatewayURL} from './gateway-url.mjs'
import {test, expect} from '@playwright/test'

// Requires the later real signed TLS issuer/launcher. Missing credentials fail visibly.
test('provider desktop handoff creates a real console session through its accessible link', async ({page, context}) => {
  test.setTimeout(60000)
  const email = process.env.LABBY_E2E_PROVIDER_EMAIL
  const password = process.env.LABBY_E2E_PROVIDER_PASSWORD
  if (!email || !password) throw new Error('Real provider issuer account variables are required')
  await page.goto('/sign-in')
  const consoleOrigin = new URL(page.url()).origin
  const start = page.getByRole('button', {name: 'Sign in with provider', exact: true})
  const status = page.locator('[data-provider-status]')
  const cancel = page.locator('[data-provider-cancel]')
  const link = page.getByRole('link', {name: 'Open provider sign-in', exact: true})
  await expect(status).toHaveAttribute('role', 'status')
  await expect(status).toHaveAttribute('aria-live', 'polite')
  expect(await status.evaluate(element => getComputedStyle(element).display)).not.toBe('none')
  await start.click()
  await expect(link).toBeVisible()
  await expect(start).toBeDisabled()
  await expect(link).toBeFocused()
  await expect(status).toContainText('return here')
  await page.keyboard.press('Escape')
  await expect(start).toBeEnabled()
  await expect(link).toBeHidden()
  await expect(cancel).toBeHidden()
  await expect(status).toContainText('reset')

  await start.click()
  await expect(start).toBeDisabled()
  await expect(link).toBeVisible()
  await expect(link).toBeFocused()
  await expect(status).toContainText('return here')
  await expect(link).toHaveAttribute('rel', 'noopener noreferrer')
  await expect(link).toHaveAttribute('target', '_blank')
  const opened = context.waitForEvent('page', {timeout: 15000})
  await link.click()
  const provider = await opened
  try {
    await provider.getByLabel('Email', {exact: true}).fill(email)
    await provider.getByLabel('Password', {exact: true}).fill(password)
    await provider.getByRole('button', {name: 'Sign in', exact: true}).click()
    await expect(page).toHaveURL(url => isGatewayURL(url, consoleOrigin), {timeout: 30000})
    await page.goto('/projects')
    await expect(page.locator('#projects-main')).toHaveAttribute('data-preference-key', /^[a-f0-9]{64}$/)
    const leaked = await page.evaluate(() => Object.keys(localStorage).some(key => /poll_token|redeem_code|code_verifier/.test(key)))
    expect(leaked).toBe(false)
  } finally {
    await provider.close()
  }
})
