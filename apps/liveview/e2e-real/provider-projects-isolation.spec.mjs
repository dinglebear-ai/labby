import {test, expect} from '@playwright/test'
import {providerSignIn} from './provider-browser.mjs'

test('two admitted provider identities isolate Projects preferences in the same browser', async ({page}) => {
  const owner = {email: 'owner@controlled.invalid', password: process.env.LABBY_E2E_PROVIDER_PASSWORD}
  const member = {email: 'member@controlled.invalid', password: process.env.LABBY_E2E_MEMBER_PASSWORD}
  if (!owner.password || !member.password) {
    throw new Error('Both genuine controlled provider accounts must be provisioned')
  }
  await providerSignIn(page, owner)
  await page.goto('/projects')
  const main = page.locator('#projects-main')
  await expect(main).toHaveAttribute('data-preference-key', /^[a-f0-9]{64}$/)
  const first = await main.getAttribute('data-preference-key')
  await page.locator('#projects-layout-grid').click()
  await page.getByRole('searchbox', {name: 'Search projects', exact: true}).fill('owner-private-query')
  await expect.poll(() => page.evaluate(key => {
    return JSON.parse(localStorage.getItem(`labby:projects:${key}:v1`) || '{}').query
  }, first)).toBe('owner-private-query')
  await page.locator('#labby-workspace > summary').click()
  await page.getByRole('button', {name: 'Sign out', exact: true}).click()
  await expect(page).toHaveURL(/\/sign-in$/)
  await providerSignIn(page, member)
  await page.goto('/projects')
  await expect(main).toHaveAttribute('data-preference-key', /^[a-f0-9]{64}$/)
  await expect(main).not.toHaveAttribute('data-preference-key', first)
  await expect(page.getByRole('searchbox', {name: 'Search projects', exact: true})).toHaveValue('')
  await expect(page.locator('#projects-layout-list')).toHaveAttribute('aria-pressed', 'true')
})

test('owner signout revokes its connected second tab while a parallel member remains valid', async ({browser}) => {
  const ownerContext = await browser.newContext()
  const memberContext = await browser.newContext()
  try {
    const owner = await ownerContext.newPage()
    const member = await memberContext.newPage()
    await providerSignIn(owner, {
      email: 'owner@controlled.invalid',
      password: process.env.LABBY_E2E_PROVIDER_PASSWORD,
    })
    await providerSignIn(member, {
      email: 'member@controlled.invalid',
      password: process.env.LABBY_E2E_MEMBER_PASSWORD,
    })
    await member.goto('/projects')
    const memberMain = member.locator('#projects-main')
    await expect(memberMain).toHaveAttribute('data-preference-key', /^[a-f0-9]{64}$/)
    const memberNamespace = await memberMain.getAttribute('data-preference-key')
    const secondOwnerTab = await ownerContext.newPage()
    await secondOwnerTab.goto('/projects')
    await expect(secondOwnerTab.locator('[data-phx-main]')).toHaveClass(/phx-connected/)
    await expect(secondOwnerTab.locator('#projects-main')).not.toHaveAttribute('data-preference-key', memberNamespace)

    await owner.locator('#labby-workspace > summary').click()
    await owner.getByRole('button', {name: 'Sign out', exact: true}).click()
    await expect(owner).toHaveURL(/\/sign-in$/)
    await expect(secondOwnerTab).toHaveURL(/\/sign-in$/, {timeout: 15000})
    await secondOwnerTab.goto('/projects')
    await expect(secondOwnerTab).toHaveURL(/\/sign-in(?:\?|$)/)

    await member.reload()
    await expect(member).toHaveURL(/\/projects$/)
    await expect(member.locator('[data-phx-main]')).toHaveClass(/phx-connected/)
    await expect(memberMain).toHaveAttribute('data-preference-key', memberNamespace)
    await member.locator('#projects-refresh').click()
    await member.locator('#projects-filter-sheet-toggle').click()
    await member.locator('#projects-workspace-team').selectOption('provider-acceptance-team')
    await expect(member.locator('#projects-workspace-team')).toHaveValue('provider-acceptance-team')
    await member.getByRole('button', {name: 'Close filters', exact: true}).click()
    await expect(member.locator('.pj-context strong')).toHaveText('provider-acceptance-team')
    await expect(member.locator('#projects-create')).toBeDisabled()
    await expect(member.locator('.pj-context')).toContainText('Your team access is read only.')
    const assignedProject = member.locator('article[data-project="bootstrap-default"]')
    await expect(assignedProject).toContainText('Provider Project')
    await expect(assignedProject).toContainText('Read only')
  } finally {
    await ownerContext.close()
    await memberContext.close()
  }
})
