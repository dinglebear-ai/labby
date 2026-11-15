import {isGatewayURL} from './gateway-url.mjs'
import {test, expect} from '@playwright/test'

// Real Labby only. The reviewed mock is a design reference, not a network dependency
// or a source of fabricated runtime values. Screenshots are review artifacts.
test('implemented screens retain the mock shell and readable desktop/mobile layouts', async ({page}, testInfo) => {
  test.setTimeout(120000)
  await page.goto('/sign-in')
  const consoleOrigin = new URL(page.url()).origin
  await page.getByLabel('Gateway credential').fill(process.env.LABBY_E2E_CREDENTIAL)
  await page.getByRole('button', {name:'Sign in', exact:true}).click()
  await expect(page).toHaveURL(url => isGatewayURL(url, consoleOrigin))
  const routes = [
    ['/gateway', '#server-list', '/gateway'],
    ['/gateway/servers/real-e2e-disabled', '#server-details-content', '/gateway'],
    ['/gateway/servers/real-e2e-disabled/catalog', '#catalog-results', '/gateway'],
    ['/gateway/upstreams/real-e2e-disabled/runtime', '#server-runtime-content', '/gateway'],
    ['/runtime', '#runtime-metrics', '/runtime'],
    ['/clients', '#clients-list, #clients-empty', '/clients'],
    ['/diagnostics', '#diagnostics-findings', '/diagnostics'],
    ['/usage', '#usage-summary', '/usage'],
    ['/activity', '#usage-calls-empty, .console-call-table', '/activity'],
    ['/logs', '#server-log-lines', '/logs'],
  ]
  for (const [width,height] of [[1280,900],[320,740]]) {
    await page.setViewportSize({width,height})
    for (const [route,ready,navigationHref] of routes) {
      await page.goto(route)
      await expect(page.locator(ready).first()).toBeVisible({timeout:15000})
      await expect(page.locator('h1')).toBeVisible()
      // Desktop groups and mobile More share the current destination contract.
      const navigation = width===1280
        ? page.locator('aside[aria-label="Labby navigation"] nav:is([aria-label="Primary navigation"], [aria-label="Workspace"], [aria-label="Operations"])')
        : page.locator('nav:is([aria-label="Labby pages"], [aria-label="More Labby pages"])')
      const current = navigation.locator('a[aria-current="page"]')
      await expect(current).toHaveCount(1)
      await expect(current).toHaveAttribute('href', navigationHref)
      const needsMore = width===320 && navigationHref!=='/gateway'
      if (needsMore) {
        await page.getByRole('button', {name:'More pages', exact:true}).click()
        await expect(page.locator('#labby-mobile-more-menu')).toBeVisible()
      }
      await expect(current).toBeVisible()
      if (needsMore) {
        await page.getByRole('button', {name:'Close more pages', exact:true}).click()
        await expect(page.locator('#labby-mobile-more-menu')).toBeHidden()
      }
      const dimensions=await page.evaluate(() => ({viewport:innerWidth, document:document.documentElement.scrollWidth}))
      expect(dimensions.document, `${route} horizontal page overflow at ${width}px`).toBeLessThanOrEqual(dimensions.viewport)
      if (width===1280 && route==='/gateway') {
        const icon=await page.locator('#servers-refresh svg').boundingBox()
        expect.soft(icon.width, 'refresh icon remains readable').toBeGreaterThanOrEqual(16)
        const connected=await page.locator('#server-list [data-state="connected"]').evaluate(el=>getComputedStyle(el).color)
        const disabled=await page.locator('#server-list [data-state="disabled"]').evaluate(el=>getComputedStyle(el).color)
        expect.soft(connected, 'connected and disabled states use distinct semantic colors').not.toBe(disabled)
      }
      if (width===1280) {
        const main=await page.locator('#console-integration').boundingBox()
        const panel=await page.locator('.console-panel').first().boundingBox()
        expect(Math.abs(panel.x-(main.x+24))).toBeLessThanOrEqual(1)
        expect(Math.abs(panel.width-(main.width-48))).toBeLessThanOrEqual(1)
      }
      const screenshot=testInfo.outputPath(`${width}-${route.replaceAll('/','-')}.png`)
      await page.screenshot({path:screenshot,fullPage:false})
      await testInfo.attach(`${width}-${route.replaceAll('/','-')}`, {path:screenshot,contentType:'image/png'})
    }
    await page.goto('/gateway')
    await page.getByRole('searchbox', {name:'Search servers'}).fill('no-such-server-visual-audit')
    await expect(page.locator('#servers-no-matches')).toBeVisible()
    await expect(page.locator('#server-list li')).toHaveCount(0)
    await page.getByRole('searchbox', {name:'Search servers'}).fill('real-e2e-disabled')
    await expect(page.getByRole('link', {name:'Real E2E Disabled',exact:true})).toBeVisible()
  }
})
