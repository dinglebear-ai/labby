import {expect} from '@playwright/test'
export async function signIn(page){await page.goto('/sign-in');await page.getByLabel('Gateway credential').fill(process.env.LABBY_E2E_CREDENTIAL);await page.getByRole('button',{name:'Sign in',exact:true}).click();await expect(page).toHaveURL(/\/gateway$/)}
export async function gateway(request,action,params={}){const r=await request.post('http://127.0.0.1:4299/v1/gateway',{headers:{Authorization:`Bearer ${process.env.LABBY_E2E_CREDENTIAL}`},data:{action,params},timeout:15000});expect(r.ok(),`${action}: HTTP ${r.status()}`).toBe(true);return r.json()}
