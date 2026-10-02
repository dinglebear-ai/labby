// Explicit test fixture: synthetic MCP, no product authentication or VM execution.
import http from 'node:http';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
const assets=process.argv[2];if(!assets||!path.isAbsolute(assets))throw Error('absolute build directory required');
const client=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../../packages/labby-tailcat-browser');
const wasm=await readFile(path.join(assets,'tailcat.wasm'));
const runtime=await readFile(path.join(assets,'wasm_exec.js'));
const sha=b=>createHash('sha256').update(b).digest('hex');
let bridge,backendRequests=0;
const backend=http.createServer(async(req,res)=>{
 if(req.url!='/mcp'||req.headers.authorization!=='Bearer fixture-only'){res.writeHead(401).end();return}
 backendRequests++;let body='';for await(const c of req){body+=c;if(body.length>4096){res.writeHead(413).end();return}}
 const m=JSON.parse(body);res.setHeader('Content-Type','application/json');
 if(m.method==='notifications/initialized'){res.writeHead(202).end();return}
 res.setHeader('Mcp-Session-Id','fixture-session');
 const result=m.method==='initialize'?{protocolVersion:'2025-03-26',capabilities:{tools:{}},serverInfo:{name:'fixture',version:'1'}}:m.method==='tools/list'?{tools:[{name:'fixture_marker',description:'Synthetic fixture only',inputSchema:{type:'object'}}]}:{content:[{type:'text',text:'tailcat-browser-marker'}]};
 res.end(JSON.stringify({jsonrpc:'2.0',id:m.id,result}));
});
await new Promise(r=>backend.listen(0,'127.0.0.1',r));
const web=http.createServer(async(req,res)=>{try{
 if(req.url==='/start'&&req.method==='POST'){
  if(bridge){res.writeHead(409).end();return}
  if(req.headers.origin!==`http://127.0.0.1:${web.address().port}`){res.writeHead(403).end();return}
  let body='';for await(const c of req){body+=c;if(body.length>1024){res.writeHead(413).end();return}}
  const {publicKey}=JSON.parse(body);
  bridge=spawn(path.join(assets,'tailcat-bridge'),[],{stdio:['pipe','pipe','ignore'],detached:true});
  const events=createInterface({input:bridge.stdout});
  const ready=new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('helper deadline')),30000);events.once('line',line=>{clearTimeout(timer);resolve(JSON.parse(line))});bridge.once('exit',()=>{clearTimeout(timer);reject(Error('helper exited'))})});
  bridge.stdin.write(JSON.stringify({version:1,type:'start',target:`127.0.0.1:${backend.address().port}`,peer:publicKey,derpMapURL:'https://tailcat.dev/derpmap.json'})+'\n');
  const event=await ready;if(event.type!=='ready')throw Error('helper failed');
  res.setHeader('Content-Type','application/json');res.end(JSON.stringify({...event,origin:'https://fixture.example',grant:'fixture-only',generation:'fixture-generation',expiresAt:Date.now()+60000,derpMapURL:'https://tailcat.dev/derpmap.json'}));return;
 }
 if(req.url==='/tailcat.wasm'){res.setHeader('Content-Type','application/wasm');res.end(wasm);return}
 if(req.url==='/wasm_exec.js'){res.setHeader('Content-Type','text/javascript');res.end(runtime);return}
 if(['/http.mjs','/transport.mjs','/wasm.mjs'].includes(req.url)){res.setHeader('Content-Type','text/javascript');res.end(await readFile(path.join(client,req.url.slice(1))));return}
 if(req.url==='/'){res.setHeader('Content-Type','text/html');res.end(`<!doctype html><meta charset="utf-8"><h1>Tailcat browser transport fixture</h1><p>Synthetic MCP; no VM or product authorization.</p><button id="run">Run transport test</button><pre id="result">Ready</pre><script type="module">
 import {loadTailcat} from '/wasm.mjs';import {TailcatClient} from '/transport.mjs';
 document.querySelector('#run').onclick=async()=>{const out=document.querySelector('#result');out.textContent='Loading';let client;try{const tc=await loadTailcat({baseURL:'/',wasmSha256:'${sha(wasm)}',runtimeSha256:'${sha(runtime)}'});const peer=tc.identity();out.textContent='Pairing fixture';const r=await fetch('/start',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({publicKey:peer.publicKey})});if(!r.ok)throw Error('fixture start');client=await TailcatClient.connect({...await r.json(),peer},{createSession:tc.createSession});out.textContent='Connecting';await client.request('initialize',{protocolVersion:'2025-03-26',capabilities:{},clientInfo:{name:'fixture',version:'1'}});await client.request('notifications/initialized');const [tools]=await Promise.all(Array.from({length:8},()=>client.request('tools/list')));const result=await client.request('tools/call',{name:'fixture_marker',arguments:{}});if(tools.result.tools[0].name!=='fixture_marker'||result.result.content[0].text!=='tailcat-browser-marker')throw Error('wrong result');out.textContent='PASS: real WASM → pinned native helper → synthetic MCP. Initialize, session, listing, marker call.';}catch(e){out.textContent='FAIL: '+e.message}finally{client?.close()}};
 </script>`);return}
 res.writeHead(404).end();
}catch{res.writeHead(500).end('fixture failure')}});
await new Promise(r=>web.listen(0,'127.0.0.1',r));
console.log(`Fixture: http://127.0.0.1:${web.address().port}`);
function stop(){if(bridge){try{process.kill(-bridge.pid,'SIGKILL')}catch{}}web.closeAllConnections();web.close();backend.closeAllConnections();backend.close();console.log(`Backend requests: ${backendRequests}`)}
process.on('SIGTERM',stop);process.on('SIGINT',stop);
setTimeout(stop,180000).unref();
