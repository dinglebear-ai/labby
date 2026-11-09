import {isGatewayURL} from './gateway-url.mjs'
import {expect} from '@playwright/test'
const gateway='http://127.0.0.1:4299'
export async function signIn(page) {
 await page.goto('/gateway')
 const consoleOrigin = new URL(page.url()).origin
 if (page.url().endsWith('/sign-in')) {
  await page.getByLabel('Gateway credential').fill(process.env.LABBY_E2E_CREDENTIAL)
  await page.getByRole('button',{name:'Sign in',exact:true}).click()
 }
 await expect(page).toHaveURL(url => isGatewayURL(url, consoleOrigin))
}
export async function gatewayAction(action,params={}) {
 const response=await fetch(`${gateway}/v1/gateway`,{method:'POST',headers:{authorization:`Bearer ${process.env.LABBY_E2E_CREDENTIAL}`,'content-type':'application/json'},body:JSON.stringify({action,params}),signal:AbortSignal.timeout(15000)})
 if(!response.ok) throw new Error(`Gateway ${action} returned ${response.status}`)
 return response.json()
}
export async function mcpRpc(method,params={},session) {
 const response=await fetch(`${gateway}/mcp`,{method:'POST',headers:{authorization:`Bearer ${process.env.LABBY_E2E_CREDENTIAL}`,'content-type':'application/json',accept:'application/json, text/event-stream',...(session?{'mcp-session-id':session}:{})},body:JSON.stringify({jsonrpc:'2.0',id:1,method,params}),signal:AbortSignal.timeout(15000)})
 if(!response.ok) throw new Error(`Real MCP ${method} returned ${response.status}`)
 const text=await response.text()
 const body=response.headers.get('content-type')?.includes('text/event-stream')?text.split('\n').filter(line=>line.startsWith('data:')).map(line=>JSON.parse(line.slice(5))).find(value=>value.id===1):JSON.parse(text)
 if(body?.error) throw new Error(`Real MCP ${method} failed: ${body.error.code}`)
 return {result:body.result,session:response.headers.get('mcp-session-id') || session}
}
async function initializeCaller(clientName) {
 const initialized=await mcpRpc('initialize',{protocolVersion:'2025-06-18',capabilities:{},clientInfo:{name:clientName,version:'1'}})
 const response=await fetch(`${gateway}/mcp`,{method:'POST',headers:{authorization:`Bearer ${process.env.LABBY_E2E_CREDENTIAL}`,'content-type':'application/json',accept:'application/json, text/event-stream',...(initialized.session?{'mcp-session-id':initialized.session}:{})},body:JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'}),signal:AbortSignal.timeout(5000)})
 if(!response.ok) throw new Error(`Real MCP initialization notification returned ${response.status}`)
 await response.arrayBuffer()
 return initialized
}
export async function callRealTool(name,args={}) {
 const initialized=await initializeCaller('liveview-real-e2e')
 const listing=await mcpRpc('tools/list',{},initialized.session)
 const tool=listing.result.tools.find(tool=>tool.name===name || tool.name===`real-e2e-mcp::${name}`)
 if(!tool) throw new Error(`Real upstream tool ${name} was not advertised`)
 const {result}=await mcpRpc('tools/call',{name:tool.name,arguments:args},initialized.session)
 return result
}
export async function upstreamOnline(online) {
 const response=await fetch('http://127.0.0.1:4300/control',{method:'POST',body:JSON.stringify({online}),signal:AbortSignal.timeout(2000)})
 if(!response.ok) throw new Error('Owned upstream control failed')
}
export async function readyCatalog() {
 await upstreamOnline(true)
 await gatewayAction('gateway.status',{name:'real-e2e-mcp'})
 // Status is the supported active refresh; retained listing never connects.
 for(let attempt=0;attempt<5;attempt++) {
  const tools=await gatewayAction('gateway.discovered_tools',{name:'real-e2e-mcp'})
  if(JSON.stringify(tools).includes('echo')) return
  if(attempt<4) await new Promise(resolve=>setTimeout(resolve,250))
 }
 throw new Error('Real upstream catalog remained empty after active status refresh')
}

export async function seedRealCalls(count) {
 if(!Number.isInteger(count) || count<1 || count>30) throw new Error('Owned call budget is 1..30')
 const initialized=await initializeCaller('liveview-real-e2e-pagination')
 const listing=await mcpRpc('tools/list',{},initialized.session)
 const tool=listing.result.tools.find(tool=>tool.name==='echo' || tool.name==='real-e2e-mcp::echo')
 if(!tool) throw new Error('Real echo tool was not advertised')
 for(let index=0;index<count;index++) {
  const {result}=await mcpRpc('tools/call',{name:tool.name,arguments:{text:`pagination evidence ${index}`}},initialized.session)
  if(result.isError) throw new Error('Real pagination call failed')
 }
}
