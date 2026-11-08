import {test,expect} from '@playwright/test'
test('public readiness leads to a themed responsive sign-in without granting access',async({page},testInfo)=>{
 await page.goto('/')
 await expect(page.locator('#gateway-status')).toHaveText('Gateway ready',{timeout:10000})
 await page.getByRole('link',{name:'Sign in',exact:true}).click()
 await expect(page).toHaveURL(/\/sign-in$/)
 await expect(page.locator('.console-auth .console-panel')).toBeVisible()
 for(const width of [1280,320]){
  await page.setViewportSize({width,height:800})
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true)
  const path=testInfo.outputPath(`sign-in-${width}.png`)
  await page.screenshot({path})
  await testInfo.attach(`sign-in-${width}`,{path,contentType:'image/png'})
 }
})
