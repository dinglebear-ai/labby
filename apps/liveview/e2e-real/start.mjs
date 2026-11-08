import {spawn,spawnSync} from 'node:child_process'
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {resolve} from 'node:path'
import {createServer as createNetServer} from 'node:net'
import {createServer} from 'node:http'
import {createHash,timingSafeEqual} from 'node:crypto'
import {BoundedDiagnostics,waitForHttp} from './launcher-support.mjs'
import {startMcpServer} from './mcp-server.mjs'
import {startRestricted} from './restricted-start.mjs'

const credential=process.env.LABBY_E2E_CREDENTIAL, controlKey=process.env.LABBY_E2E_CONTROL
if(![credential,controlKey].every(v=>/^[a-f0-9]{64}$/.test(v||'')))throw new Error('Missing isolated test credentials')
const binary=resolve(process.env.LABBY_E2E_BINARY||'../../target/debug/labby')
const root=mkdtempSync(resolve(tmpdir(),'labby-liveview-real-'))
const home=resolve(root,'labby')
mkdirSync(home,{mode:0o700})
writeFileSync(resolve(home,'config.toml'),`config_version = 1
[code_mode]
enabled = false
[[upstream]]
name = "real-e2e-disabled"
display_name = "Real E2E Disabled"
enabled = false
url = "http://127.0.0.1:1/mcp"
[[upstream]]
name = "real-e2e-mcp"
display_name = "Real E2E MCP"
enabled = true
url = "http://127.0.0.1:4300/mcp"
`,{mode:0o600})
writeFileSync(resolve(home,'.env'),`LABBY_AUTH_MODE=bearer\nLABBY_MCP_HTTP_TOKEN=${credential}\n`,{mode:0o600})
const env=Object.fromEntries(['PATH','HOME','TMPDIR','LANG','SSL_CERT_FILE','NIX_SSL_CERT_FILE'].filter(k=>process.env[k]).map(k=>[k,process.env[k]]))
const gatewayEnv={...env,LABBY_HOME:home,LABBY_LOG_DIR:resolve(root,'logs'),LABBY_LOG:'labby=info,warn',LABBY_AUTH_MODE:'bearer',LABBY_MCP_HTTP_TOKEN:credential}
const children=new Set(), sinks=[]
let stopping=false, backend, upstream, control, restricted, restarting=false, restarts=0
const receipt={pid:process.pid,sourceRevision:process.env.LABBY_E2E_SOURCE_REVISION||spawnSync('git',['rev-parse','HEAD'],{cwd:resolve('../..'),encoding:'utf8',timeout:2000}).stdout?.trim()||null,binary,binarySha256:null,ports:[4297,4298,4299,4300,4301],stages:[]}
function record(stage,result,detail={}){receipt.stages.push({stage,result,...detail});writeFileSync(resolve(root,'diagnostics.json'),JSON.stringify(receipt,null,2),{mode:0o600})}
async function stopChild(child){if(!child||child.exitCode!==null||child.signalCode!==null)return;child.expected=true;await new Promise(r=>{const timer=setTimeout(()=>process.kill(-child.pid,'SIGKILL'),5000);child.once('exit',()=>{clearTimeout(timer);r()});process.kill(-child.pid,'SIGTERM')})}
async function stop(code){if(stopping)return;stopping=true;if(restricted){try{await restricted.cleanup();record("restricted_cleanup","complete")}catch{record("restricted_cleanup","failed",{code:"E2E_CLEANUP_UNCONFIRMED"});code=1}}await Promise.all([...children].map(stopChild));if(upstream)await upstream.close();if(control)await new Promise(r=>control.close(r));for(const sink of sinks)sink.flush();record('shutdown','complete',{code});process.exit(code)}
function fail(error){if(stopping)return;record(error.stage||'launcher','failed',{code:error.code||'E2E_START_FAILED',message:String(error.message).replaceAll(credential,'[REDACTED]').replaceAll(controlKey,'[REDACTED]')});console.error(`E2E ${error.code||'E2E_START_FAILED'} at ${error.stage||'launcher'}; private redacted diagnostics: ${root}`);void stop(1)}
process.on('SIGTERM',()=>void stop(0));process.on('SIGINT',()=>void stop(0))
function launch(command,args,childEnv,name){const sink=new BoundedDiagnostics(resolve(root,`${name}.log`),[credential,controlKey]);sinks.push(sink);const child=spawn(command,args,{env:childEnv,detached:true,stdio:['ignore','pipe','pipe']});children.add(child);child.stdout.on('data',c=>sink.write(c));child.stderr.on('data',c=>sink.write(c));child.once('error',e=>fail(Object.assign(e,{stage:name})));child.once('exit',(code,signal)=>{children.delete(child);sink.flush();if(!stopping&&!child.expected)fail(Object.assign(new Error(`${name} exited (${code??signal})`),{stage:name,code:'E2E_PROCESS_EXIT'}))});return child}
async function startBackend(){backend=launch(binary,['serve','--host','127.0.0.1','--port','4299','--services','gateway,doctor,server_logs','--no-input'],gatewayEnv,'labby');await waitForHttp('http://127.0.0.1:4299/ready',{stage:'backend_ready'});record('backend_ready','complete',{pid:backend.pid})}
async function main(){
 try{receipt.binarySha256=createHash('sha256').update(readFileSync(binary)).digest('hex')}catch{throw Object.assign(new Error('Selected Labby binary cannot be read; build it or correct LABBY_E2E_BINARY'),{stage:'binary_admission',code:'E2E_BINARY_UNAVAILABLE'})}

 for(const port of receipt.ports)await new Promise((r,j)=>{const s=createNetServer();s.once('error',e=>j(Object.assign(e,{stage:'port_admission',code:'E2E_PORT_BUSY'})));s.listen(port,'127.0.0.1',()=>s.close(r))})
 upstream=await startMcpServer({port:4300});record('mcp_peer','complete')
 const bootstrap=spawnSync(binary,['setup','--bootstrap-static-owner','--no-input'],{env:gatewayEnv,encoding:'utf8',timeout:30000,maxBuffer:262144})
 const bootstrapLog=new BoundedDiagnostics(resolve(root,'bootstrap.log'),[credential,controlKey]);sinks.push(bootstrapLog);bootstrapLog.write((bootstrap.stdout||'')+(bootstrap.stderr||''))
 if(bootstrap.status!==0)throw Object.assign(new Error('Isolated owner bootstrap failed'),{stage:'owner_bootstrap',code:'E2E_BOOTSTRAP_FAILED'})
 record('owner_bootstrap','complete');await startBackend()
 restricted=await startRestricted({binary,root:process.env.LABBY_E2E_RESTRICTED_ROOT,env,launch});record('restricted_ready','complete')
 for(const sink of sinks){try{sink.secrets.push(readFileSync(resolve(process.env.LABBY_E2E_RESTRICTED_ROOT,'credential.txt'),'utf8').trim())}catch{}}
 control=createServer(async(req,res)=>{const supplied=Buffer.from(req.headers['x-test-control']||'');const expected=Buffer.from(controlKey);if(supplied.length!==expected.length||!timingSafeEqual(supplied,expected)){res.writeHead(403);res.end();return}if(req.method!=='POST'||req.url!=='/restart'){res.writeHead(404);res.end();return}if(restarting){res.writeHead(409);res.end();return}if(restarts>=3){res.writeHead(429);res.end();return}restarts++;restarting=true;try{await stopChild(backend);await startBackend();res.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify({restarted:true}))}catch(e){res.writeHead(503);res.end();fail(e)}finally{restarting=false}})
 await new Promise((r,j)=>{control.once('error',j);control.listen(4297,'127.0.0.1',r)})
 launch('mix',['run','--no-start','e2e-real/server.exs'],{...env,MIX_ENV:'test'},'liveview');await waitForHttp('http://127.0.0.1:4298/sign-in',{stage:'liveview_ready'});record('liveview_ready','complete');console.log(`Owned real Labby and MCP peer ready; private redacted diagnostics: ${root}`)
}
main().catch(fail)
