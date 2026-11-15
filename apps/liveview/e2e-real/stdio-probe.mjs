import {createHash} from 'node:crypto'

// An owned real MCP subprocess. Only digests leave its private environment.
const digest=value=>createHash('sha256').update(value).digest('hex')
const tools=[{name:'observe_private_config',description:'Observe synthetic private configuration digests',
 inputSchema:{type:'object',properties:{}},annotations:{readOnlyHint:true,destructiveHint:false}}]
const deadline=setTimeout(()=>process.exit(1),300000)
let pending=Buffer.alloc(0),requests=0
process.stdout.on('error',()=>process.exit(1))

function respond(line) {
 if(++requests>128) throw new Error('request limit')
 const input=JSON.parse(line)
 if(!input || typeof input.method!=='string') throw new Error('invalid request')
 if(input.id===undefined) return // Includes initialized and cancellation notifications.
 if(!((typeof input.id==='number' && Number.isSafeInteger(input.id)) ||
  (typeof input.id==='string' && Buffer.byteLength(input.id)<=64))) throw new Error('invalid request id')
 let result,error
 switch(input.method) {
  case 'initialize':result={protocolVersion:'2025-06-18',capabilities:{tools:{},resources:{},prompts:{}},serverInfo:{name:'private-stdio-probe',version:'1'}};break
  case 'ping':result={};break
  case 'tools/list':result={tools};break
  case 'resources/list':result={resources:[]};break
  case 'resources/templates/list':result={resourceTemplates:[]};break
  case 'prompts/list':result={prompts:[]};break
  case 'tools/call': {
   if(input.params?.name!=='observe_private_config') {error={code:-32602,message:'Unknown tool'};break}
   const value=process.env.PROJECT_CANARY_VALUE,args=process.argv.slice(2)
   const observation={valuePresent:value!==undefined,valueSha256:value===undefined?null:digest(value),
    argvSha256:digest(JSON.stringify(args)),argumentCount:args.length,
    markerPresent:args[0]==='--canary-marker' && args[1]==='labby-private-stdio'}
   result={content:[{type:'text',text:JSON.stringify(observation)}],isError:false}
   break
  }
  default:error={code:-32601,message:'Method not found'}
 }
 process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:input.id,...(error?{error}:{result})})+'\n')
}

try {
 for await(const chunk of process.stdin) {
  pending=Buffer.concat([pending,chunk])
  let newline
  while((newline=pending.indexOf(10))!==-1) {
   if(newline>16384) throw new Error('line limit')
   respond(pending.subarray(0,newline).toString('utf8'))
   pending=pending.subarray(newline+1)
  }
  if(pending.length>16384) throw new Error('line limit')
 }
 if(pending.length) throw new Error('incomplete line')
} catch {process.exitCode=1} finally {clearTimeout(deadline)}
