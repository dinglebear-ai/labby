import {test,expect} from '@playwright/test'
import {signIn,readyCatalog,upstreamOnline,callRealTool,gatewayAction} from './mcp-support.mjs'

test('real upstream discovery populates server catalog; transport failure and recovery retain truthful catalog',async({page})=>{
 await readyCatalog()
 await signIn(page)
 await page.goto('/gateway/servers/real-e2e-mcp/catalog')
 await expect(page.locator('#catalog-tools')).toContainText('echo')
 await expect(page.locator('#catalog-tools')).toContainText('fail')
 await expect(page.locator('#catalog-resources')).toContainText('test://real-e2e/readme')
 await expect(page.locator('#catalog-prompts')).toContainText('real-e2e-brief')
 await page.setViewportSize({width:320,height:800})
 await expect(page.locator('#catalog-tools')).toBeVisible()
 expect(await page.evaluate(()=>document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
 try {
  await upstreamOnline(false)
  // A failed real invocation is evidence of transport loss; cached catalog is separate.
  let failed=false
  try {const result=await callRealTool('echo',{text:'offline probe'});failed=result.isError===true} catch {failed=true}
  expect(failed).toBe(true)
  await page.reload()
  await expect(page.locator('#catalog-tools')).toContainText('echo')
 } finally {await upstreamOnline(true)}
 await gatewayAction('gateway.reload')
 await readyCatalog()
 await page.reload()
 await expect(page.locator('#catalog-tools')).toContainText('echo')
})
