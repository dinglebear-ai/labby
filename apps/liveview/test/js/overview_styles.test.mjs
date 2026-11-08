import {test} from 'node:test'
import assert from 'node:assert/strict'
import {readFileSync} from 'node:fs'
import {fileURLToPath} from 'node:url'
import {spawnSync} from 'node:child_process'
import {chromium} from '@playwright/test'
const root=fileURLToPath(new URL('../..',import.meta.url))
test('native Overview styles leave a legacy read page unchanged regardless of import order', {timeout:20000}, async()=>{
 const workspace=readFileSync(new URL('../../assets/css/workspace.css',import.meta.url),'utf8').replace(/@import\s+["']\.\/overview-dashboard\.css["'];?/g,'')
 // Ask the pinned dependency for its configured executable and native target.
 // Inherit MIX_ENV/MIX_BUILD_PATH and never download or compile during this probe.
 const resolved=spawnSync('mix',['run','--no-compile','--no-start','-e','IO.puts(Esbuild.bin_path())'],{cwd:root,encoding:'utf8',maxBuffer:8192,timeout:10000})
 assert.equal(resolved.status,0,resolved.stderr||resolved.error?.message)
 const executable=resolved.stdout.trim()
 assert.ok(executable&&!/[\r\n]/.test(executable),'Mix must return one Esbuild executable path')
 const baseline=spawnSync(executable,['--loader=css','--bundle','--sourcefile=workspace-style-probe.css'],{cwd:`${root}/assets/css`,input:workspace,encoding:'utf8',maxBuffer:2*1024*1024,timeout:10000})
 assert.equal(baseline.status,0,baseline.stderr)
 const css=readFileSync(new URL('../../assets/css/overview-dashboard.css',import.meta.url),'utf8')
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH})
 try{
  const page=await browser.newPage({viewport:{width:1440,height:900}})
  await page.route('**/*',route=>route.abort())
  await page.setContent(`<style>${baseline.stdout}</style><div class="labby-overview"><main id="console-integration"><h1>Runtime</h1><section class="console-panel"><h2>Host measurements</h2></section></main></div>`)
  const measure=()=>page.evaluate(()=>{
   const main=document.getElementById('console-integration'),h=main.querySelector('h1'),panel=main.querySelector('.console-panel')
   const style=getComputedStyle(main),heading=getComputedStyle(h),content=getComputedStyle(panel)
   return {padding:style.padding,maxWidth:style.maxWidth,font:heading.fontSize,headingMargin:heading.margin,panelPadding:content.padding,panelBorder:content.borderTopWidth}
  })
  const before=await measure()
  await page.addStyleTag({content:css})
  assert.deepEqual(await measure(),before)
  await page.evaluate(()=>document.getElementById('console-integration').classList.add('ov-main','ov-compact-page'))
  assert.notEqual((await measure()).maxWidth,before.maxWidth)
  await page.setViewportSize({width:320,height:900})
  await page.evaluate(()=>{
   document.getElementById('console-integration').insertAdjacentHTML('beforeend','<section id="ov-recent-calls" class="ov-panel"><div class="ov-table-scroll"><table class="ov-table ov-calls-table"><thead><tr><th>Calls</th></tr></thead><tbody id="ov-call-rows"><tr><td><button class="ov-call-name">Actual call</button></td><td>Server</td><td>Caller</td><td>ok</td><td>53ms</td><td>10:00 UTC</td></tr></tbody></table></div></section>')
  })
  assert.equal(await page.locator('#ov-call-rows tr').evaluate(el=>getComputedStyle(el).display),'grid')
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true)

 }finally{await browser.close()}
})
