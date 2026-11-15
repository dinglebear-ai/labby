import {test,expect} from '@playwright/test'
import {signIn} from './session.mjs'
test.beforeEach(async({page})=>signIn(page))
test('inventory reads real configured upstreams',async({page})=>{await expect(page.getByRole('link',{name:'Real E2E Disabled',exact:true})).toBeVisible();await expect(page.locator('#servers-error')).toHaveCount(0)})
test('server details preserve disabled configuration',async({page})=>{await page.getByRole('link',{name:'Real E2E Disabled',exact:true}).click();await expect(page.locator('#server-details-content')).toContainText('Disabled')})
test('server runtime reads the real process snapshot',async({page})=>{await page.goto('/gateway/upstreams/real-e2e-disabled/runtime');await expect(page.locator('#server-runtime-content')).toContainText('Disabled')})
test('clients report real observations without asserting liveness',async({page})=>{await page.goto('/clients');await expect(page.locator('#clients-empty, #clients-list li').first()).toBeVisible();await expect(page.locator('#clients-error')).toHaveCount(0)})
test('diagnostics runs real read-only checks',async({page})=>{await page.goto('/diagnostics');await expect(page.locator('#diagnostics-findings li').first()).toBeVisible({timeout:15000});await expect(page.locator('#diagnostics-error')).toHaveCount(0)})
test('host metrics complete the actual sample',async({page})=>{await page.goto('/runtime');await expect(page.locator('#runtime-metrics')).toBeVisible({timeout:15000});await expect(page.locator('#runtime-error')).toHaveCount(0)})
test('usage reads exact real aggregates',async({page})=>{await page.goto('/usage');await expect(page.locator('#usage-summary')).toBeVisible();await expect(page.locator('#usage-error')).toHaveCount(0)})
test('activity displays the retained real page',async({page})=>{await page.goto('/activity');await expect(page.locator('#usage-calls-empty, #usage-calls-table tbody tr').first()).toBeVisible();await expect(page.locator('#usage-calls-error')).toHaveCount(0)})
test('logs pause actual polling',async({page})=>{await page.goto('/logs');await expect(page.locator('.console-log-line').first()).toBeVisible();await page.getByRole('button',{name:'Pause',exact:true}).click();await expect(page.locator('#server-log-lines')).toHaveAttribute('data-following','false')})
