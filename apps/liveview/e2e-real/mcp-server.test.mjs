import {test} from 'node:test'
import assert from 'node:assert/strict'
import {startMcpServer} from './mcp-server.mjs'
test('real upstream implements discovery, calls, and recoverable transport failure', async () => {
 const server = await startMcpServer({port:0})
 const rpc = async (method, params={}) => fetch(server.url,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:1,method,params})})
 try {
  assert.equal((await (await rpc('initialize')).json()).result.serverInfo.name,'real-e2e-mcp')
  assert.equal((await (await rpc('tools/list')).json()).result.tools.length,2)
  assert.equal((await (await rpc('resources/list')).json()).result.resources.length,1)
  assert.equal((await (await rpc('prompts/list')).json()).result.prompts.length,1)
  assert.equal((await (await rpc('tools/call',{name:'echo',arguments:{text:'actual call'}})).json()).result.content[0].text,'actual call')
  assert.equal((await (await rpc('tools/call',{name:'fail'})).json()).result.isError,true)
  await fetch(server.controlUrl,{method:'POST',body:JSON.stringify({online:false})})
  assert.equal((await rpc('tools/list')).status,503)
  await fetch(server.controlUrl,{method:'POST',body:JSON.stringify({online:true})})
  assert.equal((await rpc('tools/list')).status,200)
 } finally {await server.close()}
})
