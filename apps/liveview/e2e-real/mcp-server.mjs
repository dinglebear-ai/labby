import {createServer} from 'node:http'

// A real protocol peer, not a replacement for Labby's API or usage store.
export async function startMcpServer({port=4300,host='127.0.0.1'}={}) {
 if (host !== '127.0.0.1') throw new Error('Test upstream must stay on loopback')
 let online = true
 const tools = [
  {name:'echo',description:'Echo a deterministic test message',inputSchema:{type:'object',properties:{text:{type:'string'}},required:['text']},annotations:{readOnlyHint:true,destructiveHint:false}},
  {name:'fail',description:'Return a completed tool error',inputSchema:{type:'object',properties:{}},annotations:{readOnlyHint:true,destructiveHint:false}},
 ]
 const server = createServer(async (req,res) => {
  const send = (status,value) => {res.writeHead(status,{'content-type':'application/json'});res.end(value === undefined ? undefined : JSON.stringify(value))}
  if (!['/mcp','/control'].includes(req.url)) return send(404,{error:'not found'})
  if (req.url === '/mcp' && !online) return send(503,{error:'upstream offline'})
  if (req.method === 'GET') return send(405,{error:'SSE not required by this stateless peer'})
  if (req.method === 'DELETE') return send(200,{})
  if (req.method !== 'POST') return send(405,{error:'POST required'})
  let body=''
  try {
   for await (const chunk of req) {body+=chunk;if(Buffer.byteLength(body)>16384) return send(413,{error:'body limit'})}
   const input=JSON.parse(body)
   if (req.url==='/control') {if(typeof input.online!=='boolean') return send(400,{});online=input.online;return send(200,{online})}
   if (input.id === undefined) return send(202)
   let result
   switch(input.method) {
    case 'initialize': result={protocolVersion:input.params?.protocolVersion || '2025-06-18',capabilities:{tools:{},resources:{},prompts:{}},serverInfo:{name:'real-e2e-mcp',version:'1.0.0'}};break
    case 'ping':result={};break
    case 'tools/list':result={tools};break
    case 'resources/list':result={resources:[{uri:'test://real-e2e/readme',name:'Real E2E Readme',mimeType:'text/plain'}]};break
    case 'resources/templates/list':result={resourceTemplates:[]};break
    case 'prompts/list':result={prompts:[{name:'real-e2e-brief',description:'A deterministic test prompt'}]};break
    case 'prompts/get':result={messages:[{role:'user',content:{type:'text',text:'Summarize the real E2E evidence.'}}]};break
    case 'resources/read':result={contents:[{uri:'test://real-e2e/readme',mimeType:'text/plain',text:'Real upstream resource'}]};break
    case 'tools/call':
     if (!tools.some(tool=>tool.name===input.params?.name)) return send(200,{jsonrpc:'2.0',id:input.id,error:{code:-32602,message:'Unknown tool'}})
     result={content:[{type:'text',text:input.params.name==='fail'?'Deliberate tool error':String(input.params.arguments?.text ?? '')}],isError:input.params.name==='fail'};break
    default:return send(200,{jsonrpc:'2.0',id:input.id,error:{code:-32601,message:'Method not found'}})
   }
   send(200,{jsonrpc:'2.0',id:input.id,result})
  } catch {send(400,{error:'Invalid JSON request'})}
 })
 server.requestTimeout=5000;server.headersTimeout=5000
 await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(port,host,resolve)})
 const origin=`http://${host}:${server.address().port}`
 return {url:`${origin}/mcp`,controlUrl:`${origin}/control`,close:()=>new Promise((resolve,reject)=>{server.close(error=>error?reject(error):resolve());server.closeIdleConnections()})}
}
