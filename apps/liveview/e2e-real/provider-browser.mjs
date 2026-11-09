import {expect} from '@playwright/test'

export async function providerSignIn(page, account) {
  await page.goto('/sign-in')
  await page.getByRole('button', {name: 'Sign in with provider', exact: true}).click()
  const link = page.getByRole('link', {name: 'Open provider sign-in', exact: true})
  await expect(link).toHaveAttribute('rel', 'noopener noreferrer')
  const opened = page.context().waitForEvent('page', {timeout: 15000})
  await link.click()
  const provider = await opened
  try {
    await provider.getByLabel('Email', {exact: true}).fill(account.email)
    await provider.getByLabel('Password', {exact: true}).fill(account.password)
    await provider.getByRole('button', {name: 'Sign in', exact: true}).click()
    await expect(page).toHaveURL(/\/gateway$/, {timeout: 30000})
    await expect(page.locator('[data-phx-main]')).toHaveClass(/phx-connected/)
  } finally {
    await provider.close()
  }
}
