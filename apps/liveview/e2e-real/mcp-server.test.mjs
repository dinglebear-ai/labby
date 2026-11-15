import {test} from 'node:test'
import assert from 'node:assert/strict'
import {createHash} from 'node:crypto'
import {createServer} from 'node:net'
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

test('private peer authenticates its digest-only bounded wire observations and releases its port', async () => {
 const key='a'.repeat(64), secret='synthetic-wire-value'
 const server=await startMcpServer({port:0,probeControlKey:key})
 const origin=new URL(server.url).origin
 const probe=()=>fetch(`${origin}/probe`,{headers:{'x-test-control':key}})
 try {
  assert.equal((await fetch(`${origin}/probe`)).status,403)
  assert.equal((await fetch(`${origin}/probe`,{headers:{'x-test-control':'b'.repeat(64)}})).status,403)
  assert.equal((await fetch(server.controlUrl,{method:'POST',body:'{"online":false}'})).status,403)
  for(let index=0;index<20;index++) {
   const response=await fetch(server.url,{method:'POST',headers:{'x-labby-literal':secret,'x-labby-env':secret,authorization:`Bearer ${secret}`,'x-other-secret':secret},body:JSON.stringify({jsonrpc:'2.0',id:index,method:index===19?secret:'ping',params:{secret}})})
   assert.equal(response.status,200)
  }
  const response=await probe()
  assert.equal(response.status,200)
  const raw=await response.text()
  assert.equal(raw.includes(secret),false)
  const result=JSON.parse(raw)
  assert.equal(result.observations.length,16)
  assert.equal(result.sequence,20)
  assert.deepEqual(result.observations.map(row=>row.sequence),Array.from({length:16},(_,index)=>index+5))
  const last=result.observations.at(-1)
  assert.equal(last.method,'unknown')
  assert.deepEqual(last.headers,{
   'x-labby-literal':{present:true,sha256:createHash('sha256').update(secret).digest('hex')},
   'x-labby-env':{present:true,sha256:createHash('sha256').update(secret).digest('hex')},
   authorization:{present:true,sha256:createHash('sha256').update(`Bearer ${secret}`).digest('hex')},
  })
  assert.deepEqual(Object.keys(last).sort(),['headers','method','sequence'])
  assert.equal((await fetch(server.url,{method:'POST',body:'x'.repeat(8193)})).status,413)
  assert.equal((await (await probe()).json()).sequence,20)
  await fetch(server.url,{method:'POST',body:JSON.stringify({jsonrpc:'2.0',id:21,method:'ping'})})
  assert.deepEqual((await (await probe()).json()).observations.at(-1).headers.authorization,{present:false,sha256:null})
  for(let index=21;index<128;index++) {
   assert.equal((await fetch(server.url,{method:'POST',body:'{"jsonrpc":"2.0","id":1,"method":"ping"}'})).status,200)
  }
  assert.equal((await fetch(server.url,{method:'POST',body:'{"jsonrpc":"2.0","id":1,"method":"ping"}'})).status,429)
  assert.equal((await (await probe()).json()).sequence,128)
  let finalStatus
  for(let index=0;index<512;index++) finalStatus=(await probe()).status
  assert.equal(finalStatus,429)
 } finally {await server.close()}
 const socket=createServer()
 await new Promise((resolve,reject)=>{socket.once('error',reject);socket.listen(Number(new URL(origin).port),'127.0.0.1',resolve)})
 await new Promise(resolve=>socket.close(resolve))
})

test('private peer requires a strict control key and default peer has no probe endpoint', async () => {
 let invalidServer, rejection
 try {invalidServer=await startMcpServer({port:0,probeControlKey:'short'})} catch(error) {rejection=error}
 if(invalidServer) await invalidServer.close()
 assert.match(rejection?.message || '',/control key/i)
 const server=await startMcpServer({port:0})
 try {assert.equal((await fetch(`${new URL(server.url).origin}/probe`)).status,404)} finally {await server.close()}
})
