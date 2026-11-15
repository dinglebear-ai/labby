import {test} from 'node:test'
import assert from 'node:assert/strict'
import {spawnSync} from 'node:child_process'
import {createHash} from 'node:crypto'
import {mkdtempSync,rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join,resolve} from 'node:path'

const helper=resolve('e2e-real/stdio-probe.mjs')
const digest=value=>createHash('sha256').update(value).digest('hex')
const rpc=(id,method,params={})=>JSON.stringify({jsonrpc:'2.0',...(id===undefined?{}:{id}),method,params})+'\n'

function run(input,value,args=['--canary-marker','labby-private-stdio','space separated','','quote"literal']) {
 const home=mkdtempSync(join(tmpdir(),'labby-stdio-unit-'))
 try {
  return spawnSync(process.execPath,[helper,...args],{input,encoding:'utf8',timeout:5000,maxBuffer:65536,
   env:{HOME:home,TMPDIR:home,...(value===undefined?{}:{PROJECT_CANARY_VALUE:value})}})
 } finally {rmSync(home,{recursive:true,force:true})}
}

test('real stdio child discovers and returns only private env and exact argv digests, then exits on EOF',()=>{
 const secret='synthetic-stdio-private-value'
 const args=['--canary-marker','labby-private-stdio','space separated','','quote"literal']
 const input=rpc(1,'initialize',{protocolVersion:'2025-06-18'})+rpc(undefined,'notifications/initialized')+
  rpc(2,'tools/list')+rpc(3,'tools/call',{name:'observe_private_config'})+
  rpc(4,'resources/list')+rpc(5,'resources/templates/list')+rpc(6,'prompts/list')+
  rpc(undefined,'notifications/cancelled',{requestId:3})
 const result=run(input,secret,args)
 assert.equal(result.status,0)
 assert.equal(result.stderr,'')
 assert.equal(result.stdout.includes(secret),false)
 const rows=result.stdout.trim().split('\n').map(line=>JSON.parse(line))
 assert.equal(rows.length,6)
 assert.equal(rows[0].result.protocolVersion,'2025-06-18')
 assert.equal(rows[1].result.tools[0].name,'observe_private_config')
 assert.deepEqual(JSON.parse(rows[2].result.content[0].text),{
  valuePresent:true,valueSha256:digest(secret),argvSha256:digest(JSON.stringify(args)),argumentCount:5,markerPresent:true,
 })
 assert.deepEqual(rows.slice(3).map(row=>row.result),[{resources:[]},{resourceTemplates:[]},{prompts:[]}])
})

test('stdio wire distinguishes absent and replaced values without exposing either value',()=>{
 const input=rpc(1,'tools/call',{name:'observe_private_config'})
 const absent=run(input)
 const replaced=run(input,'replacement-private-value')
 assert.equal(absent.status,0)
 assert.equal(replaced.status,0)
 const observation=result=>JSON.parse(JSON.parse(result.stdout).result.content[0].text)
 assert.equal(observation(absent).valuePresent,false)
 assert.equal(observation(absent).valueSha256,null)
 assert.equal(observation(replaced).valueSha256,digest('replacement-private-value'))
 assert.equal(replaced.stdout.includes('replacement-private-value'),false)
})

test('stdio helper bounds lines and request count, rejects unknown tools and writes no private diagnostics',()=>{
 const unknown=run(rpc(1,'tools/call',{name:'private-unknown-tool'}))
 assert.equal(unknown.status,0)
 assert.equal(JSON.parse(unknown.stdout).error.code,-32602)
 assert.equal(unknown.stdout.includes('private-unknown-tool'),false)
 for(const input of ['x'.repeat(16385),'{invalid}\n',rpc(1,'ping').repeat(129)]) {
  const result=run(input)
  assert.equal(result.status,1)
  assert.equal(result.stderr,'')
  assert.ok(Buffer.byteLength(result.stdout)<=16384)
 }
})

test('static config generates bounded private header values, preserves an explicit synthetic value and rejects oversized input',()=>{
 const config=resolve('playwright.config.mjs')
 const execute=value=>spawnSync(process.execPath,['--input-type=module','-e',
  `import {rmSync} from 'node:fs'; await import(${JSON.stringify(config)}); process.stdout.write(JSON.stringify({valid:/^[A-Za-z0-9_-]{16,128}$/.test(process.env.LABBY_E2E_HEADER_VALUE||''),preserved:process.env.LABBY_E2E_HEADER_VALUE==='synthetic_canary_value',root:Boolean(process.env.LABBY_E2E_PRIVATE_ROOT)})); for(const key of ['LABBY_E2E_PRIVATE_ROOT','LABBY_E2E_RESTRICTED_ROOT']) if(process.env[key]) rmSync(process.env[key],{recursive:true,force:true})`],
 {encoding:'utf8',timeout:5000,env:{PATH:process.env.PATH,...(value===undefined?{}:{LABBY_E2E_HEADER_VALUE:value})}})
 const fresh=execute()
 assert.equal(fresh.status,0)
 assert.deepEqual(JSON.parse(fresh.stdout),{valid:true,preserved:false,root:true})
 assert.equal(JSON.parse(execute('synthetic_canary_value').stdout).preserved,true)
 assert.notEqual(execute('x'.repeat(129)).status,0)
})
