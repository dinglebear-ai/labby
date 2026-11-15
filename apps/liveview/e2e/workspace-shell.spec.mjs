import {test, expect} from '@playwright/test'
test('native shell geometry, navigation and keyboard menus', async ({page}) => {
  test.setTimeout(45000)
  await page.goto('/sign-in')
  await page.getByLabel('Gateway credential').fill('fixture-only')
  await page.getByRole('button', {name: 'Sign in', exact: true}).click()
  await page.setViewportSize({width: 1440, height: 1000})
  await expect(page.locator('#labby-workspace-shell')).toBeVisible()
  expect((await page.locator('.ov-sidebar').boundingBox()).width).toBe(206)
  expect((await page.locator('.ov-topbar').boundingBox()).height).toBe(52)
  await page.getByRole('button', {name: 'Collapse navigation', exact: true}).click()
  await expect(page.locator('.ov-sidebar')).toHaveAttribute('data-collapsed', 'true')
  await page.reload()
  await expect(page.locator('.ov-sidebar')).toHaveAttribute('data-collapsed', 'true')
  await page.getByRole('button', {name: 'Expand navigation', exact: true}).click()
  await page.locator('#labby-nav-runtime').click()
  await expect(page).toHaveURL(/\/runtime$/)
  await expect(page.locator('#labby-nav-runtime')).toHaveAttribute('aria-current', 'page')
  for (const width of [740, 320]) {
    await page.setViewportSize({width, height: 800})
    await expect(page.locator('.ov-sidebar')).toBeHidden()
    await expect(page.locator('.labby-mobile-nav')).toHaveCSS('display', 'grid')
    for (const control of await page.locator('.labby-mobile-nav > :is(a, button)').all()) {
      const bounds = await control.boundingBox()
      expect(bounds).not.toBeNull()
      expect(bounds.x).toBeGreaterThanOrEqual(0)
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(width)
      expect(bounds.width).toBeGreaterThanOrEqual(44)
    }
    await page.getByRole('button', {name: 'More pages', exact: true}).click()
    await expect(page.locator('#labby-mobile-more-menu')).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.locator('#labby-mobile-more-menu')).toBeHidden()
    await expect(page.locator('#labby-mobile-more')).toBeFocused()
    await page.locator('#labby-mobile-profile > summary').click()
    await expect(page.locator('#labby-mobile-profile-menu')).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.locator('#labby-mobile-profile-menu')).toBeHidden()
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  }
})

test('native base styles keep links, profile spacing and controls faithful', async ({page}) => {
  await page.goto('/sign-in')
  await page.getByLabel('Gateway credential').fill('fixture-only')
  await page.getByRole('button', {name: 'Sign in', exact: true}).click()
  await page.setViewportSize({width: 1440, height: 1000})
  for (const selector of ['.ov-brand', '.ov-sidebar .ov-nav-item', '#labby-workspace-menu > a']) {
    await expect(page.locator(selector).first()).toHaveCSS('text-decoration-line', 'none')
  }
  await page.locator('#labby-workspace > summary').click()
  await expect(page.locator('#labby-workspace-menu')).toBeVisible()
  for (const selector of ['.lb-profile-menu-label', '.lb-profile-note']) {
    await expect(page.locator(`#labby-workspace-menu ${selector}`)).toHaveCSS('margin-top', '0px')
    await expect(page.locator(`#labby-workspace-menu ${selector}`)).toHaveCSS('margin-bottom', '0px')
  }
  await page.locator('.ov-topbar > div').first().click()
  await expect(page.locator('#labby-workspace-menu')).toBeHidden()

  // A bounded CSS specimen tests native classes even with an empty Projects inventory.
  // It also catches resets that accidentally erase explicit action/read-model styling.
  await page.locator('#labby-workspace-shell').evaluate(shell => {
    const specimen = document.createElement('section')
    specimen.id = 'workspace-style-specimen'
    specimen.className = 'lb-operations-page'
    specimen.innerHTML = '<nav class="pj-scopes"><button aria-pressed="false">Scope</button><button aria-pressed="true">Selected</button></nav><button class="pj-project-name">Project</button><button class="lb-action">Action</button><div class="console-panel"><button>Legacy action</button></div>'
    shell.querySelector(".ov-body").append(specimen)
  })
  try {
    for (const selector of ['.pj-scopes button[aria-pressed=false]', '.pj-project-name']) {
      const control = page.locator(`#workspace-style-specimen ${selector}`)
      await expect(control).toHaveCSS('border-top-width', '0px')
      await expect(control).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
    }
    await expect(page.locator('#workspace-style-specimen .pj-scopes button[aria-pressed=true]')).toHaveCSS('border-bottom-width', '2px')
    for (const selector of ['.lb-action', '.console-panel button']) {
      const control = page.locator(`#workspace-style-specimen ${selector}`)
      await expect(control).toHaveCSS('border-top-width', '1px')
      await expect(control).toHaveCSS('border-top-style', 'solid')
      await expect(control).not.toHaveCSS('background-color', 'rgba(0, 0, 0, 0)')
    }
    await page.emulateMedia({reducedMotion: 'reduce'})
    await expect(page.locator('.labby-connection-dot')).toHaveCSS('animation-name', 'none')
    await expect(page.locator('#workspace-style-specimen .lb-action')).toHaveCSS('transition-duration', '0s')
  } finally {
    await page.locator('#workspace-style-specimen').evaluate(element => element.remove())
  }
})
