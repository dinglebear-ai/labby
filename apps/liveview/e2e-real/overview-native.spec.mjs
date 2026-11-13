import {test,expect} from '@playwright/test'
import {signIn,readyCatalog,callRealTool} from './mcp-support.mjs'

test('native Overview uses real telemetry, scoped drawers and caller-scoped card layout',async({page},info)=>{
 test.setTimeout(90000)
 await readyCatalog()
 await callRealTool('echo',{text:'overview-native-evidence'})
 await callRealTool('fail')
 await signIn(page);await page.goto('/overview')
 await expect(page.locator('#overview-usage')).toBeVisible()
 await expect(page.locator('#ov-rankings')).toContainText('echo')
 await expect(page.locator('#overview-refresh')).toBeEnabled({timeout:20000})
 for(const window of ['1h','24h','7d','30d']){
  await page.locator(`#ov-window-${window}`).click()
  await expect(page.locator(`#ov-window-${window}`)).toHaveAttribute('aria-pressed','true')
  await expect(page.locator('#overview-usage')).toBeVisible()
  await expect(page.locator('#overview-refresh')).toBeEnabled({timeout:20000})
  if (window === '1h') {
   await page.locator('#ov-bucket-0').click()
   await expect(page.locator('#ov-inspector')).toContainText('Retained calls')
   await expect(page.locator('#ov-inspector [role=alert]')).toHaveCount(0)
   await page.locator('#ov-inspector-close').click()
   await expect(page.locator('#ov-inspector')).toHaveCount(0)
  }
 }
 await page.locator('#ov-call-rows tr button').first().click()
 await expect(page.locator('#ov-call-attribution dt')).toContainText([
  'Caller identity', 'Client', 'Client version', 'Surface', 'Agent', 'Task', 'Subject scoped',
 ])
 await page.locator('#ov-inspector-close').click()
 await expect(page.locator('#ov-inspector')).toHaveCount(0)
 await page.locator('#ov-ranking-slowest').click()
 await expect(page.locator('#ov-rankings')).toContainText('Mean elapsed')
 await page.locator('#ov-rankings-open-0').click()
 await expect(page.locator('#ov-inspector')).toBeVisible()
 await expect(page.locator('#ov-inspector')).toContainText('Retained calls')
 await page.keyboard.press('Escape');await expect(page.locator('#ov-inspector')).toHaveCount(0)
 await expect(page.locator('#ov-rankings-open-0')).toBeFocused()
 await page.locator('#ov-activity-callers').click();await expect(page.locator('#ov-rankings')).toBeVisible()
 await page.locator('#ov-activity-servers').click();await expect(page.locator('#ov-rankings')).toContainText('real-e2e-mcp')
 await page.locator('#ov-chart-mode').selectOption('outcomes')
 await page.locator('#ov-bucket-0').click();await expect(page.locator('#ov-inspector')).toContainText('Time bucket')
 await page.locator('#ov-inspector-close').click();await expect(page.locator('#ov-inspector')).toHaveCount(0)
 await page.locator('#ov-view-options > summary').click();await page.locator('#ov-customize').click()
 await expect(page.locator('#ov-card-grid')).toHaveAttribute('data-editing','true')
 await page.locator('#ov-card-calls').focus();await page.keyboard.press('Alt+ArrowUp')
 await page.getByRole('button',{name:'Resize Gateway traffic',exact:true}).click()
 await expect(page.locator('#ov-card-traffic')).toHaveAttribute('data-wide','true')
 const key=await page.locator('#console-integration').getAttribute('data-preference-key')
 await expect.poll(()=>page.evaluate(key=>JSON.parse(localStorage.getItem(`labby:overview:${key}:v1`)||'{}').widths?.traffic,key)).toBe(true)
 await page.reload();await expect(page.locator('#ov-card-traffic')).toHaveAttribute('data-wide','true')
 await page.locator('#ov-view-options > summary').click();await page.locator('#ov-layout-reset').click()
 await expect(page.locator('#ov-operational-layout')).toBeVisible()
 for(const width of [1440,740,320]){
  await page.setViewportSize({width,height:900})
  await expect.poll(()=>page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true)
  await expect(page.locator('#ov-window-30d')).toBeVisible()
  await page.screenshot({path:info.outputPath(`overview-native-${width}.png`),fullPage:true})
 }
 await page.emulateMedia({reducedMotion:'reduce'})
 await expect.poll(()=>page.locator('#console-integration').evaluate(el=>getComputedStyle(el).animationDuration)).toBe('0s')
})
