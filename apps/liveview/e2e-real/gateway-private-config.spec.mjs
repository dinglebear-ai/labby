import {test,expect} from '@playwright/test'
import {createHash,randomBytes} from 'node:crypto'
import {readFileSync} from 'node:fs'
import {join} from 'node:path'
import {signIn,gatewayAction,mcpRpc} from './mcp-support.mjs'
import {runPrivateScenario} from './private-scenario.mjs'

// UI Gateway charges are source-derived estimates, separate from bounded direct
// MCP/probe traffic and Auth preflights. Parent acceptance must measure runtime
// Gateway actions. Each scenario reserves eight of forty actions for cleanup.
test.describe.configure({mode:'serial'})
test.use({screenshot:'off',trace:'off',video:'off'})
const digest=value=>createHash('sha256').update(value).digest('hex')
const sentinel='real-e2e-disabled'

function budget() {
 const receipt={gatewayEstimated:0,gatewayDirect:0,protocolAndProbe:0,charges:[]}
 function charge(count,label,{direct=false,cleanup=false}={}) {
  if(receipt.gatewayEstimated+receipt.gatewayDirect+count>(cleanup?40:32)) throw new Error('Private scenario Gateway action budget exhausted')
  receipt[direct?'gatewayDirect':'gatewayEstimated']+=count
  receipt.charges.push({label,count,kind:direct?'gateway-direct':'gateway-ui-estimate'})
 }
 return {receipt,charge,
  action:async(action,params={},cleanup=false)=>{charge(1,action,{direct:true,cleanup});return gatewayAction(action,params)},
  protocol:async(operation)=>{
   if(receipt.protocolAndProbe>=80) throw new Error('Private scenario MCP/probe request budget exhausted')
   receipt.protocolAndProbe++
   return operation()
  },
 }
}

async function settled(page) {
 await expect(page.locator('#gateway-add')).toBeEnabled({timeout:35000})
 await expect(page.locator('#servers-refresh')).toBeEnabled()
 await expect(page.locator('#gm-receipt')).toHaveCount(0)
 await expect(page.locator('#gm-dialog')).toHaveCount(0)
}

async function edit(page,name,limit,step) {
 limit.charge(1,'UI edit fresh get')
 await step('edit-open',()=>page.getByRole('button',{name:`Edit ${name}`,exact:true}).first().click())
 await step('edit-loaded',()=>expect(page.locator('#gm-name')).toHaveValue(name))
}

async function privateInput(locator,value) {
 try {await locator.fill(value)} catch {throw new Error('Private synthetic input could not be filled')}
}

async function authentication(page) {
 const section=page.locator('details.gm-section').filter({has:page.locator('#gm-bearer-env')})
 if(await section.getAttribute('open')===null) await section.locator('summary').first().click()
}

async function privateMap(page,field,intent,value,step) {
 await step(`${field}-authentication-open`,()=>authentication(page))
 const section=page.locator('details.gm-private-map').filter({has:page.locator(`#gm-${field}-intent`)})
 if(await section.getAttribute('open')===null) await step(`${field}-map-open`,()=>section.locator('summary').click())
 await step(`${field}-intent-${intent}`,()=>page.locator(`#gm-${field}-intent`).selectOption(intent))
 if(intent==='replace') await step(`${field}-private-input`,()=>privateInput(page.locator(`#gm-${field}`),JSON.stringify(value)))
}

async function save(page,limit,step) {
 limit.charge(3,'UI write, affected get, fleet list')
 await step('save-submit-once',()=>page.locator('#gm-save').click()) // Submitted writes are never replayed.
 await step('save-settled',()=>settled(page))
 await step('save-accepted',()=>expect(page.locator('#gm-notice')).toContainText('accepted'))
}

async function hidden(page,view,values) {
 const projection=JSON.stringify(view),dom=await page.content()
 expect(values.some(value=>projection.includes(value))).toBe(false)
 expect(values.some(value=>dom.includes(value))).toBe(false)
 expect(await page.evaluate(values=>values.some(value=>JSON.stringify(localStorage).includes(value)),values)).toBe(false)
}

async function caller(limit) {
 const initialized=await limit.protocol(()=>mcpRpc('initialize',{protocolVersion:'2025-06-18',capabilities:{},clientInfo:{name:'private-config-canary',version:'1'}}))
 await limit.protocol(async()=>{
  const response=await fetch('http://127.0.0.1:4299/mcp',{method:'POST',headers:{authorization:`Bearer ${process.env.LABBY_E2E_CREDENTIAL}`,
   'content-type':'application/json',accept:'application/json, text/event-stream',...(initialized.session?{'mcp-session-id':initialized.session}:{})},
   body:JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'}),signal:AbortSignal.timeout(5000)})
  if(!response.ok) throw new Error(`Private MCP initialization notification returned ${response.status}`)
  await response.arrayBuffer()
 })
 return initialized.session
}

async function ownedTool(limit,session,name,localName) {
 const qualified=`${name}::${localName}`
 for(let attempt=0;attempt<5;attempt++) {
  const listing=await limit.protocol(()=>mcpRpc('tools/list',{},session))
  const tool=listing.result.tools.find(tool=>tool.name===qualified)
  if(tool) {
   const response=await limit.protocol(()=>mcpRpc('tools/call',{name:tool.name,arguments:localName==='echo'?{text:'private-wire-canary'}:{}},session))
   expect(response.result.isError===true).toBe(false)
   return response.result
  }
  if(attempt<4) await new Promise(resolve=>setTimeout(resolve,250))
 }
 throw new Error('Generated upstream qualified tool was not advertised within five reads')
}

async function probe(limit) {
 return limit.protocol(async()=>{
  const response=await fetch('http://127.0.0.1:4302/probe',{headers:{'x-test-control':process.env.LABBY_E2E_CONTROL},signal:AbortSignal.timeout(5000)})
  if(!response.ok) throw new Error(`Private wire probe returned ${response.status}`)
  return response.json()
 })
}

async function wireAfter(limit,sequence,method,expected) {
 for(let attempt=0;attempt<5;attempt++) {
  const state=await probe(limit)
  const observation=state.observations.filter(row=>row.sequence>sequence && row.method===method).at(-1)
  if(observation) {
   expect(observation.headers).toEqual(Object.fromEntries(Object.entries(expected).map(([name,value])=>[
    name,{present:value!==null,sha256:value===null?null:digest(value)},
   ])))
   return
  }
  if(attempt<4) await new Promise(resolve=>setTimeout(resolve,250))
 }
 throw new Error('Actual private upstream request was absent after five probe reads')
}

async function cleanup(page,name,limit,sentinelConfig) {
 // Inspect uncertain writes first. A fresh read can authorize this separate,
 // name-confirmed deletion. It never authorizes replay of the previous write.
 if(await page.locator('#gm-close').count()) await page.locator('#gm-close').click()
 const rows=await limit.action('gateway.list',{},true)
 if(rows.some(row=>row.name===name && row.source==='custom_gateway')) {
  if(await page.locator('#gm-receipt').count()) {
   limit.charge(1,'UI cleanup reload fleet list',{cleanup:true})
   await page.reload()
   await settled(page)
  }
  limit.charge(1,'UI cleanup delete fresh get',{cleanup:true})
  await page.getByRole('button',{name:`Delete ${name}`,exact:true}).first().click()
  await expect(page.locator('#gm-remove-form')).toBeVisible()
  await page.getByLabel('Type the exact server ID to confirm').fill(name)
  limit.charge(3,'UI cleanup remove, affected get, fleet list',{cleanup:true})
  await page.locator('#gm-remove').click()
  await settled(page)
 }
 const after=await limit.action('gateway.list',{},true)
 expect(after.some(row=>row.name===name)).toBe(false)
 if(sentinelConfig) expect((await limit.action('gateway.get',{name:sentinel},true)).config).toEqual(sentinelConfig)
}

test('private HTTP literal replacement, preserve, clear, env header and derived bearer reach the real upstream',async({page},info)=>{
 test.setTimeout(300000)
 const limit=budget(),name=`private-http-${randomBytes(5).toString('hex')}`
 const first=randomBytes(24).toString('hex'),second=randomBytes(24).toString('hex')
 const draft=randomBytes(24).toString('hex'),bearer=randomBytes(24).toString('hex')
 const env=process.env.LABBY_E2E_HEADER_VALUE
 if(!/^[A-Za-z0-9_-]{16,128}$/.test(env||'')) throw new Error('Owned synthetic header environment is unavailable')
 const secrets=[first,second,draft,bearer,env]
 let sentinelConfig
 await runPrivateScenario({page,info,accounting:limit.receipt,cleanup:()=>cleanup(page,name,limit,sentinelConfig)},async step=>{
  sentinelConfig=(await step('sentinel-read',()=>limit.action('gateway.get',{name:sentinel}))).config
  limit.charge(1,'UI initial fleet list')
  await step('sign-in',()=>signIn(page))
  await step('liveview-connected',()=>page.locator('[data-phx-session].phx-connected').waitFor())
  await step('initial-fleet-ready',()=>settled(page))
  const session=await step('mcp-initialize',()=>caller(limit))
  await step('add-open',()=>page.locator('#gateway-add').click())
  await step('draft-name',()=>page.locator('#gm-name').fill(name))
  await step('draft-endpoint',()=>page.locator('#gm-url').fill('http://127.0.0.1:4302/mcp'))
  await privateMap(page,'headers','replace',{'x-labby-literal':first},step)
  await privateMap(page,'headers_env','replace',{'x-labby-env':'PROJECT_CANARY_HEADER_VALUE'},step)
  await expect(page.locator('#gm-bearer-env')).toHaveValue('')
  await step('bearer-private-input',()=>privateInput(page.locator('#gm-bearer-value'),bearer))
  await save(page,limit,step)

  async function verify(literal,environment) {
   const view=await limit.action('gateway.get',{name})
   expect(/^[A-Z_][A-Z0-9_]*$/.test(view.config.bearer_token_env||'')).toBe(true)
   await hidden(page,view,secrets)
   const baseline=(await probe(limit)).sequence
   await limit.action('gateway.status',{name})
   await ownedTool(limit,session,name,'echo')
   await wireAfter(limit,baseline,'tools/call',{'x-labby-literal':literal,'x-labby-env':environment,authorization:`Bearer ${bearer}`})
  }

  await step('verify-first-http',()=>verify(first,env))
  await edit(page,name,limit,step)
  await privateMap(page,'headers','replace',{'x-labby-literal':second},step)
  await save(page,limit,step)
  await step('verify-preserved-http',()=>verify(second,env))

  await edit(page,name,limit,step)
  await privateMap(page,'headers','replace',{'x-labby-literal':draft},step)
  await expect(page.locator('#gm-test')).toHaveText('Test saved configuration')
  const baseline=(await probe(limit)).sequence
  limit.charge(1,'UI explicit saved-name private probe')
  await step('saved-private-probe',()=>page.locator('#gm-test').click())
  await expect(page.locator('#gm-test-result')).toContainText('Probe connected',{timeout:35000})
  await wireAfter(limit,baseline,'initialize',{'x-labby-literal':second,'x-labby-env':env,authorization:`Bearer ${bearer}`})
  await privateMap(page,'headers','preserve',undefined,step)
  await step('display-intent',()=>page.locator('#gm-display-intent').selectOption('replace'))
  await step('display-http-label',()=>page.locator('#gm-display-name').fill('Private HTTP preserve canary'))
  await save(page,limit,step)
  await step('verify-preserved-http',()=>verify(second,env))

  await edit(page,name,limit,step)
  await privateMap(page,'headers','clear',undefined,step)
  await privateMap(page,'headers_env','clear',undefined,step)
  await save(page,limit,step)
  await step('verify-cleared-http',()=>verify(null,null))
 })
})

test('private stdio environment replace, preserve and clear retain exact real subprocess argv boundaries',async({page},info)=>{
 test.setTimeout(300000)
 const limit=budget(),name=`private-stdio-${randomBytes(5).toString('hex')}`
 const first=randomBytes(24).toString('hex'),second=randomBytes(24).toString('hex')
 const fixture=JSON.parse(readFileSync(join(process.env.LABBY_E2E_PRIVATE_ROOT,'stdio-fixture.json'),'utf8'))
 let sentinelConfig
 await runPrivateScenario({page,info,accounting:limit.receipt,cleanup:()=>cleanup(page,name,limit,sentinelConfig)},async step=>{
  sentinelConfig=(await step('sentinel-read',()=>limit.action('gateway.get',{name:sentinel}))).config
  limit.charge(1,'UI initial fleet list')
  await step('sign-in',()=>signIn(page))
  await step('liveview-connected',()=>page.locator('[data-phx-session].phx-connected').waitFor())
  await step('initial-fleet-ready',()=>settled(page))
  const session=await step('mcp-initialize',()=>caller(limit))
  await step('add-open',()=>page.locator('#gateway-add').click())
  await step('draft-name',()=>page.locator('#gm-name').fill(name))
  await step('draft-stdio-select',()=>page.getByLabel('stdio',{exact:true}).check())
  await step('draft-command',()=>page.locator('#gm-command').fill(fixture.command))
  await step('draft-arguments',()=>page.locator('#gm-args').fill(JSON.stringify(fixture.args)))
  await privateMap(page,'env','replace',{PROJECT_CANARY_VALUE:first},step)
  await save(page,limit,step)

  async function verify(value,args) {
   const view=await limit.action('gateway.get',{name})
   await hidden(page,view,[first,second])
   await limit.action('gateway.status',{name})
   const result=await ownedTool(limit,session,name,'observe_private_config')
   const observation=JSON.parse(result.content.find(part=>part.type==='text').text)
   expect(observation).toEqual({valuePresent:value!==null,valueSha256:value===null?null:digest(value),
    argvSha256:digest(JSON.stringify(args.slice(1))),argumentCount:args.length-1,markerPresent:true})
  }

  await step('verify-first-stdio',()=>verify(first,fixture.args))
  const replacement=[...fixture.args,'--flag=two words','a\\b','']
  await edit(page,name,limit,step)
  await step('replace-arguments',()=>page.locator('#gm-args').fill(JSON.stringify(replacement)))
  await privateMap(page,'env','replace',{PROJECT_CANARY_VALUE:second},step)
  await save(page,limit,step)
  await step('verify-preserved-stdio',()=>verify(second,replacement))
  await edit(page,name,limit,step)
  await step('display-intent',()=>page.locator('#gm-display-intent').selectOption('replace'))
  await step('display-stdio-label',()=>page.locator('#gm-display-name').fill('Private stdio preserve canary'))
  await save(page,limit,step)
  await step('verify-preserved-stdio',()=>verify(second,replacement))
  await edit(page,name,limit,step)
  await privateMap(page,'env','clear',undefined,step)
  await save(page,limit,step)
  await step('verify-cleared-stdio',()=>verify(null,replacement))
 })
})
