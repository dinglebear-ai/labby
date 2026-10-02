// Explicit live acceptance: the native Rust fixture owns auth, control and cleanup.
import https from 'node:https';
import http from 'node:http';
import {readFile, chmod} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {fileURLToPath, pathToFileURL} from 'node:url';
import path from 'node:path';
const [assets,socket,credential,root,modulePath,name]=process.argv.slice(2);
if(![assets,socket,credential,root,modulePath].every(p=>p&&path.isAbsolute(p))||!/^labby-tailcat-[a-f0-9]{32}$/.test(name||''))throw Error('explicit owned fixture paths and VM identity required');
const client=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../../packages/labby-tailcat-browser');
const wasm=await readFile(path.join(assets,'tailcat.wasm'));
const runtime=await readFile(path.join(assets,'wasm_exec.js'));
const sha=b=>createHash('sha256').update(b).digest('hex');
const key=path.join(root,'browser-fixture.key'),cert=path.join(root,'browser-fixture.crt');
execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',key,'-out',cert,'-days','1','-subj','/CN=127.0.0.1','-addext','subjectAltName=IP:127.0.0.1'],{stdio:'ignore',timeout:10000});
await chmod(key,0o600);
const source=(await readFile(credential,'utf8')).trim();
async function control(route,body) {
 return new Promise((resolve,reject)=>{
  const bytes=Buffer.from(JSON.stringify(body));
  const req=http.request({socketPath:socket,path:route,method:'POST',headers:{Host:'localhost','Content-Type':'application/json','Content-Length':bytes.length}},res=>{
   let length=0;const chunks=[];
   res.on('data',chunk=>{length+=chunk.length;if(length>16384){req.destroy();reject(Error('control response budget'));return;}chunks.push(chunk);});
   res.on('end',()=>{if(res.statusCode===204){resolve();return;}if(res.statusCode!==200){reject(Error(`native control ${route} rejected (${res.statusCode})`));return;}
    try{resolve(JSON.parse(Buffer.concat(chunks).toString()));}catch{reject(Error('invalid control response'));}});
  });
  req.setTimeout(40000,()=>req.destroy(Error('native control deadline')));req.on('error',reject);req.end(bytes);
 });
}
let origin,delivery,started=false,browser;
const web=https.createServer({key:await readFile(key),cert:await readFile(cert)},async(req,res)=>{
 try {
  if(req.url==='/start'&&req.method==='POST') {
   if(started||req.headers.origin!==origin){res.writeHead(403).end();return;}started=true;
   let body='';for await(const chunk of req){body+=chunk;if(Buffer.byteLength(body)>1024)throw Error('pairing budget');}
   const {publicKey}=JSON.parse(body);
   const pairing={origin,peer:publicKey,upstream:'msb'};
   const prepared=await control('/prepare',{...pairing,source_credential:source});
   delivery=await control('/approve',{...pairing,id:prepared.id,nonce:prepared.nonce});
   res.setHeader('Content-Type','application/json');res.end(JSON.stringify(delivery));return;
  }
  if(req.url==='/tailcat.wasm'){res.setHeader('Content-Type','application/wasm');res.end(wasm);return;}
  if(req.url==='/wasm_exec.js'){res.setHeader('Content-Type','text/javascript');res.end(runtime);return;}
  if(['/http.mjs','/transport.mjs','/wasm.mjs'].includes(req.url)){res.setHeader('Content-Type','text/javascript');res.end(await readFile(path.join(client,req.url.slice(1))));return;}
  if(req.url==='/'){res.setHeader('Content-Type','text/html');res.end('<!doctype html><meta charset="utf-8"><h1>Owned native VM acceptance</h1><p>Private fixture; no production service.</p>');return;}
  res.writeHead(404).end();
 } catch {res.writeHead(500).end('native pairing fixture failed');}
});
await new Promise(r=>web.listen(0,'127.0.0.1',r));origin=`https://127.0.0.1:${web.address().port}`;
let evidence={ok:false,vm:name,networkDisabled:true,hostMounts:false};
try {
 const {chromium}=await import(pathToFileURL(modulePath).href);
 browser=await chromium.launch({headless:true});
 const context=await browser.newContext({ignoreHTTPSErrors:true}); // Owned self-signed loopback fixture only.
 const page=await context.newPage();await page.goto(origin);
 page.setDefaultTimeout(180000);
 evidence=await page.evaluate(async({name,wasmSha,runtimeSha})=>{
  const {loadTailcat}=await import('/wasm.mjs');const {TailcatClient}=await import('/transport.mjs');
  let client,createAttempted=false;const result={ok:false,vm:name,networkDisabled:true,hostMounts:false};
  const deadline=setTimeout(()=>client?.close(),175000);
  try {
   const tc=await loadTailcat({baseURL:'/',wasmSha256:wasmSha,runtimeSha256:runtimeSha});const peer=tc.identity();
   const response=await fetch('/start',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({publicKey:peer.publicKey})});
   if(!response.ok)throw Error('native_pairing_failed');
   const packet=await response.json();if(packet.origin!==location.origin||packet.peer!==peer.publicKey)throw Error('binding_mismatch');
   client=await TailcatClient.connect({...packet,peer},{createSession:tc.createSession});
   await client.request('initialize',{protocolVersion:'2025-03-26',capabilities:{},clientInfo:{name:'native-vm-acceptance',version:'1'}});
   await client.request('notifications/initialized');
   const listing=await client.request('tools/list');const tools=listing.result?.tools||[];
   const tool=action=>{const matches=tools.filter(t=>t.name.endsWith(action));if(matches.length!==1)throw Error('restricted_catalog_mismatch');return matches[0].name;};
   const call=async(action,args)=>{const r=await client.request('tools/call',{name:tool(action),arguments:args});
    const text=r.result?.content?.find(c=>c.type==='text')?.text;const body=text&&JSON.parse(text);
    if(r.error||r.result?.isError||!body?.ok){result.failure={action,rpcCode:r.error?.code,toolError:r.result?.isError,bodyKeys:Object.keys(body||{}),kind:body?.error?.code||body?.kind||body?.error?.kind};throw Error(`${action}_failed`);}return body.data;};
   result.tools=tools.map(t=>t.name);
   if(tools.some(t=>/^(gateway|access|setup|fs)$/.test(t.name)))throw Error('operator_tool_exposed');
   const existing=await call('sandbox_list',{});if(existing.some(vm=>vm.name===name))throw Error('owned_name_collision');
   createAttempted=true;
   await call('sandbox_create',{name,rootfs:{kind:'oci',reference:'docker.io/library/ubuntu@sha256:a6757b311b671e9379ac0548b5abf7a7e68d33bff51f6044bbd60f68f781489a',pullPolicy:'never',upperSizeMib:128},cpus:1,memoryMib:128,network:{disabled:true},process:{user:'nobody',workdir:'/tmp',labels:{'labby-tailcat-run':name}},lifecycle:{maxDurationSecs:120,idleTimeoutSecs:60}});
   const output=await call('sandbox_exec',{name,command:'/bin/sh',args:['-c','test "$(uname -s)" = Linux && test "$(id -u)" != 0 && printf tailcat-native-vm-marker'],user:'nobody',timeoutMs:15000,maxBytes:1024,treatNonZeroAsError:true});
   if(output.exitCode!==0||output.stdout!=='tailcat-native-vm-marker')throw Error('guest_identity_marker_failed');
   result.marker=output.stdout;result.guestNonRoot=true;result.guestKernel='Linux';
   await call('sandbox_remove',{name,force:true});createAttempted=false;
   const remaining=await call('sandbox_list',{});if(remaining.some(vm=>vm.name===name))throw Error('owned_guest_remains');
   result.cleanup='verified_absent';result.ok=true;
  } catch(error) {result.error=String(error.message).replace(/[^a-zA-Z0-9_ -]/g,'').slice(0,160);}
  finally {
   if(createAttempted&&client){try{const listing=await client.request('tools/list');const tool=listing.result.tools.find(t=>t.name.endsWith('sandbox_remove'));await client.request('tools/call',{name:tool.name,arguments:{name,force:true}});}catch{result.cleanup='native_compensation_required';}}
   clearTimeout(deadline);client?.close();
  }
  return result;
 },{name,wasmSha:sha(wasm),runtimeSha:sha(runtime)});
} catch {evidence.error='browser_fixture_failed';}
finally {
 await browser?.close();
 if(delivery){try{await control('/stop',{id:delivery.id});}catch{evidence.ok=false;evidence.nativeCleanup='unconfirmed';}}
 web.closeAllConnections();await new Promise(resolve=>web.close(resolve));
}
process.stdout.write(JSON.stringify(evidence)+'\n');if(!evidence.ok)process.exitCode=1;
