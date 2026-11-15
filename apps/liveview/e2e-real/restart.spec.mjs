import {test,expect} from '@playwright/test'
import {signIn,gateway} from './session.mjs'
test('configuration survives a real daemon restart and old browser authority is rejected',async({page,request})=>{
 await signIn(page)
 await gateway(request,'gateway.update',{name:'real-e2e-disabled',patch:{display_name:'Persisted restart label'}})
 const before=await gateway(request,'gateway.server.get',{id:'real-e2e-disabled'})
 expect(before.enabled).toBe(false)
 const r=await request.post('http://127.0.0.1:4297/restart',{headers:{'x-test-control':process.env.LABBY_E2E_CONTROL},timeout:45000})
 expect(r.status()).toBe(200)
 await page.goto('/clients')
 await expect(page).toHaveURL(/\/sign-in$/)
 await signIn(page)
 const after=await gateway(request,'gateway.server.get',{id:'real-e2e-disabled'})
 expect(after.id).toBe(before.id);expect(after.display_name).toBe('Persisted restart label');expect(after.enabled).toBe(false)
 await gateway(request,'gateway.update',{name:'real-e2e-disabled',patch:{display_name:'Real E2E Disabled'}})
 await page.goto('/gateway')
 await page.getByRole('link',{name:'Real E2E Disabled',exact:true}).click()
 await expect(page.locator('#server-details-content')).toContainText('Disabled')
})
