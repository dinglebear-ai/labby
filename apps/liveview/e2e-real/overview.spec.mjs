import {test,expect} from '@playwright/test'
import {signIn} from './session.mjs'

test('overview shows real scoped summaries and preserves responsive navigation',async({page},testInfo)=>{
 await page.goto('/overview')
 await expect(page).toHaveURL(/\/sign-in$/)
 await signIn(page)
 await page.getByRole('link',{name:'Overview',exact:true}).click()
 await expect(page.locator('#overview-inventory')).toContainText('Configured')
 await expect(page.locator('#overview-usage')).toContainText('Total calls')
 await page.getByRole('button',{name:'Refresh overview',exact:true}).click()
 await expect(page.locator('#overview-usage')).toBeVisible()
 await expect.poll(()=>page.locator('#overview-refresh').evaluate(el=>getComputedStyle(el).backgroundColor)).not.toBe('rgb(239, 239, 239)')
 await expect(page.locator('[aria-current=page]')).toHaveCount(1)
 for(const width of [1280,320]){
  await page.setViewportSize({width,height:900})
  await expect.poll(()=>page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true)
  await page.screenshot({path:testInfo.outputPath(`overview-${width}.png`),fullPage:true})
 }
 await page.getByRole('link',{name:'View servers',exact:false}).click()
 await expect(page).toHaveURL(/\/gateway$/)
 await expect(page.getByRole('link',{name:'Real E2E Disabled',exact:true})).toBeVisible()
})
