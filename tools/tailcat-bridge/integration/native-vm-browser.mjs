// Explicit live acceptance: the native Rust fixture owns auth, control and cleanup.
import https from 'node:https';
import http from 'node:http';
import {readFile,writeFile,unlink, chmod} from 'node:fs/promises';
import {execFileSync,spawn} from 'node:child_process';
import net from 'node:net';
import os from 'node:os';
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
let origin,delivery,started=false,browser,depotProcess,depotPort,fixturePage,stage='boot';
const depotRoot=process.env.LABBY_TAILCAT_ACCEPTANCE_DEPOT;
if(depotRoot&&!path.isAbsolute(depotRoot))throw Error('explicit Depot checkout required');
const web=https.createServer({key:await readFile(key),cert:await readFile(cert)},async(req,res)=>{
 try {
  if(depotPort){const proxy=http.request({hostname:'127.0.0.1',port:depotPort,path:req.url,method:req.method,headers:{...req.headers,'x-forwarded-proto':'https'}},upstream=>{res.writeHead(upstream.statusCode,upstream.headers);upstream.pipe(res);});proxy.on('error',()=>res.writeHead(502).end());req.pipe(proxy);return;}
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
 if(depotRoot){
  stage='depot_start';
  const reservation=net.createServer();await new Promise(r=>reservation.listen(0,'127.0.0.1',r));depotPort=reservation.address().port;await new Promise(r=>reservation.close(r));
  const tokenFile=path.join(root,'depot-reader-token');await unlink(tokenFile).catch(error=>{if(error.code!=='ENOENT')throw error;});
  const boot=path.join(root,'depot-fixture.exs');
  await writeFile(boot,`endpoint = Application.fetch_env!(:depot, DepotWeb.Endpoint)
Application.put_env(:depot, DepotWeb.Endpoint, Keyword.merge(endpoint, http: [ip: {127,0,0,1}, port: ${depotPort}]))
{:ok, _} = Application.ensure_all_started(:depot)
{:ok, token, _} = Depot.Auth.Token.create("tailcat-owned-fixture", [Depot.Auth.read_scope()])
File.write!(System.fetch_env!("TAILCAT_TOKEN_FILE"), token)
File.chmod!(System.fetch_env!("TAILCAT_TOKEN_FILE"), 0o600)
Process.sleep(:infinity)
`,{mode:0o600});
  depotProcess=spawn('mise',['exec','--','mix','run','--no-start',boot],{cwd:depotRoot,stdio:'ignore',env:{PATH:process.env.PATH,MISE_DATA_DIR:process.env.MISE_DATA_DIR||path.join(os.homedir(),'.local/share/mise'),HOME:root,MIX_ENV:'dev',DEPOT_DATA_DIR:path.join(root,'depot-data'),DEPOT_PORT:String(depotPort),DEPOT_PUBLIC_HOST:'127.0.0.1',DEPOT_PUBLIC_SCHEME:'https',DEPOT_PUBLIC_PORT:String(web.address().port),DEPOT_AUTH_MODE:'bearer',DEPOT_TRUST_FORWARDED_PROTO:'true',DEPOT_TAILCAT_ENABLED:'true',DEPOT_TAILCAT_ORIGIN:origin,DEPOT_TAILCAT_CONNECT_ORIGINS:['https://tailcat.dev',...['tc301a.ipn.dev','tc302a.ipn.dev','tc303a.ipn.dev','tc304a.ipn.dev'].flatMap(host=>['https://'+host,'wss://'+host])].join(','),TAILCAT_TOKEN_FILE:tokenFile}});
  let ready=false;for(let i=0;i<300;i++){if(depotProcess.exitCode!==null){evidence.depotExitCode=depotProcess.exitCode;break;}try{await readFile(tokenFile);await new Promise((resolve,reject)=>{const probe=http.get({hostname:'127.0.0.1',port:depotPort,path:'/ui/login'},response=>{response.resume();response.statusCode===200?resolve():reject(Error('not ready'));});probe.setTimeout(1000,()=>probe.destroy());probe.once('error',reject);});ready=true;break;}catch{await new Promise(r=>setTimeout(r,100));}}if(!ready)throw Error('owned_depot_start_failed');
  web.on('upgrade',(req,socket,head)=>{const upstream=net.connect(depotPort,'127.0.0.1',()=>{const headers={...req.headers,'x-forwarded-proto':'https'};upstream.write(`${req.method} ${req.url} HTTP/1.1\r\n`+Object.entries(headers).map(([k,v])=>`${k}: ${v}`).join('\r\n')+'\r\n\r\n');if(head.length)upstream.write(head);socket.pipe(upstream);upstream.pipe(socket);});upstream.on('error',()=>socket.destroy());socket.on('close',()=>upstream.destroy());});
 }
 browser=await chromium.launch({headless:true});
 const context=await browser.newContext({ignoreHTTPSErrors:true}); // Owned self-signed loopback fixture only.
 const page=await context.newPage();fixturePage=page;
 page.on('websocket',socket=>{evidence.relaySockets??=[];const url=new URL(socket.url());evidence.relaySockets.push({origin:url.origin,path:url.pathname});socket.on('socketerror',()=>{evidence.relaySocketError=true;});});
 page.on('console',message=>{if(message.type()==='error'&&message.text().includes('Content Security Policy'))evidence.cspBlocked=true;});
 page.on('requestfailed',request=>{evidence.requestFailures??=[];if(evidence.requestFailures.length<4){const url=new URL(request.url());evidence.requestFailures.push({origin:url.origin,path:url.pathname,code:request.failure()?.errorText});}});
 page.on('response',response=>{if(response.status()>=400){evidence.httpFailures??=[];if(evidence.httpFailures.length<8)evidence.httpFailures.push({path:new URL(response.url()).pathname,status:response.status()});}});
 if(depotRoot){
  stage='depot_login_page';await page.goto(origin+'/ui/login');stage='depot_login_form';await page.locator('input[name="token"]').fill((await readFile(path.join(root,'depot-reader-token'),'utf8')).trim());stage='depot_login_submit';await Promise.all([page.waitForURL(url=>url.pathname==='/'),page.locator('form[action="/ui/login"] button').click()]);
  stage='depot_page';await page.goto(origin+'/ui/sandboxes');await page.waitForSelector('[data-phx-main].phx-connected');
  stage='depot_prepare';await page.locator('[name="upstream"]').fill('msb');await page.locator('[data-tailcat="prepare"]').click();
  await page.waitForFunction(()=>/--pairing-id [A-Za-z0-9_-]{43}/.test(document.querySelector('[data-tailcat="command"]')?.textContent||''));
  const command=await page.locator('[data-tailcat="command"]').textContent();
  const pairingId=command.match(/--pairing-id ([A-Za-z0-9_-]{43})/)[1];
  const code=(await page.locator('[data-tailcat="code"]').textContent()).trim();
  if(!/^[A-Za-z0-9_-]{43}$/.test(code))throw Error('pairing_code_shape');
  const exchangeURL=origin+'/ui/tailcat/pairings/'+pairingId+'/exchange';
  stage='native_exchange';const fetched=await context.request.get(exchangeURL,{headers:{'X-Labby-Pairing-Code':code},timeout:10000});
  if(fetched.status()!==200)throw Error('native_exchange_denied');
  const exchange=await fetched.json();const request=exchange.request;
  if(exchange.id!==pairingId||request.origin!==origin)throw Error('native_exchange_binding');
  const fingerprint=sha(Buffer.from(JSON.stringify(['labby.tailcat.public-request/v1',pairingId,request.origin,request.peer,request.upstream])));
  if((await page.locator('[data-tailcat="fingerprint"]').textContent()).trim()!==fingerprint)throw Error('fingerprint_mismatch');
  stage='native_approve';const prepared=await control('/prepare',{origin:request.origin,peer:request.peer,upstream:request.upstream,source_credential:source});
  delivery=await control('/approve',{origin:request.origin,peer:request.peer,upstream:request.upstream,id:prepared.id,nonce:prepared.nonce,exchange_id:pairingId});
  if(!delivery.packet?.ciphertext||'address' in delivery||'grant' in delivery)throw Error('plaintext_delivery_exposed');
  stage='encrypted_deposit';const deposited=await context.request.post(exchangeURL,{headers:{'X-Labby-Pairing-Code':code},data:delivery.packet,timeout:10000});
  if(deposited.status()!==204)throw Error('encrypted_deposit_denied');
  await page.waitForFunction(()=>document.querySelector('[data-tailcat="status"]')?.textContent==='ready',{},{timeout:45000});
  evidence.encryptedPairingVerified=true;
 }else{await page.goto(origin);}
 page.setDefaultTimeout(180000);
 stage='vm_workflow';evidence=await page.evaluate(async({name,wasmSha,runtimeSha,depot})=>{
  const {loadTailcat}=await import(depot?'/assets/tailcat/wasm.mjs':'/wasm.mjs');const {TailcatClient}=await import(depot?'/assets/tailcat/transport.mjs':'/transport.mjs');
  let client,createAttempted=false;const result={ok:false,vm:name,networkDisabled:true,hostMounts:false,encryptedPairingVerified:Boolean(depot)};
  const deadline=setTimeout(()=>client?.close(),175000);
  try {
   if(depot){const el=document.getElementById('tailcat-sandbox');client=window.liveSocket.owner(el).getHook(el).client;if(!client)throw Error('depot_hook_not_connected');result.depotLiveViewConnected=true;}else{
   const tc=await loadTailcat({baseURL:'/',wasmSha256:wasmSha,runtimeSha256:runtimeSha});const peer=tc.identity();
   const response=await fetch('/start',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({publicKey:peer.publicKey})});
   if(!response.ok)throw Error('native_pairing_failed');
   const packet=await response.json();if(packet.origin!==location.origin||packet.peer!==peer.publicKey)throw Error('binding_mismatch');
   client=await TailcatClient.connect({...packet,peer},{createSession:tc.createSession});
   await client.request('initialize',{protocolVersion:'2025-03-26',capabilities:{},clientInfo:{name:'native-vm-acceptance',version:'1'}});
   await client.request('notifications/initialized');
   }
   const listing=await client.request('tools/list');const tools=listing.result?.tools||[];
   const tool=action=>{const matches=tools.filter(t=>t.name.endsWith(action));if(matches.length!==1)throw Error('restricted_catalog_mismatch');return matches[0].name;};
   const call=async(action,args)=>{const r=await client.request('tools/call',{name:tool(action),arguments:args});
    const text=r.result?.content?.find(c=>c.type==='text')?.text;const body=text&&JSON.parse(text);
    if(action==='sandbox_remove'&&body?.ok&&r.result?._meta?.['labby.tailcat.atomic_cleanup']!==1)throw Error('atomic_cleanup_not_used');
    if(r.error||r.result?.isError||!body?.ok){result.failure={action,rpcCode:r.error?.code,toolError:r.result?.isError,bodyKeys:Object.keys(body||{}),kind:body?.error?.code||body?.kind||body?.error?.kind,message:String(body?.error?.message||'').replace(/(?:lby_[A-Za-z0-9_-]+|Bearer\s+\S+)/g,'[redacted]').slice(0,512)};throw Error(`${action}_failed`);}return body.data;};
   result.tools=tools.map(t=>t.name);
   if(tools.some(t=>/^(gateway|access|setup|fs)$/.test(t.name)))throw Error('operator_tool_exposed');
   for(const action of ['sandbox_remove','sandbox_exec','sandbox_inspect']) {
    let denied;
    try {denied=await client.request('tools/call',{name:tool(action),arguments:{name:'existing-user-vm',...(action==='sandbox_exec'?{command:'/bin/true'}:{force:true})}});}
    catch(error){if(!Number.isInteger(error.code))throw error;denied={error:{code:error.code}};}
    const text=denied.result?.content?.find(c=>c.type==='text')?.text;
    const body=text&&JSON.parse(text);
    if(!denied.error&&!denied.result?.isError&&!body?.error)throw Error('foreign_access_allowed');
   }
   result.foreignCleanupDenied=true;result.foreignExecutionDenied=true;result.foreignInspectionDenied=true;
   const existing=await call('sandbox_list',{});if(existing.some(vm=>vm.name===name))throw Error('owned_name_collision');
   createAttempted=true;
   await call('sandbox_create',{name,rootfs:{kind:'oci',reference:'docker.io/library/ubuntu@sha256:a6757b311b671e9379ac0548b5abf7a7e68d33bff51f6044bbd60f68f781489a',pullPolicy:'never',upperSizeMib:128},cpus:1,memoryMib:128,network:{disabled:true},process:{user:'nobody',workdir:'/tmp',labels:{'labby-tailcat-run':name}},lifecycle:{maxDurationSecs:120,idleTimeoutSecs:60}});
   const inspected=await call('sandbox_inspect',{name});
   if(inspected.name!==name||inspected.config?.network?.enabled!==false||!Array.isArray(inspected.config?.mounts)||inspected.config.mounts.length!==0)throw Error('guest_isolation_config_mismatch');
   result.isolationConfigVerified=true;
   const output=await call('sandbox_exec',{name,command:'/bin/sh',args:['-c','test "$(uname -s)" = Linux && test "$(id -u)" != 0 && printf tailcat-native-vm-marker'],user:'nobody',timeoutMs:15000,maxBytes:1024,treatNonZeroAsError:true});
   if(output.exitCode!==0||output.stdout!=='tailcat-native-vm-marker')throw Error('guest_identity_marker_failed');
   result.marker=output.stdout;result.guestNonRoot=true;result.guestKernel='Linux';
   await call('sandbox_remove',{name,force:true});result.atomicCleanupVerified=true;createAttempted=false;
   const remaining=await call('sandbox_list',{});if(remaining.some(vm=>vm.name===name))throw Error('owned_guest_remains');
   result.cleanup='verified_absent';result.ok=true;
  } catch(error) {result.error=String(error.message).replace(/[^a-zA-Z0-9_ -]/g,'').slice(0,160);}
  finally {
   if(createAttempted&&client){try{const listing=await client.request('tools/list');const tool=listing.result.tools.find(t=>t.name.endsWith('sandbox_remove'));await client.request('tools/call',{name:tool.name,arguments:{name,force:true}});}catch{result.cleanup='native_compensation_required';}}
   clearTimeout(deadline);client?.close();
  }
  return result;
 },{name,wasmSha:sha(wasm),runtimeSha:sha(runtime),depot:Boolean(depotRoot)});
} catch {evidence.error='browser_fixture_failed';evidence.stage=stage;if(fixturePage){const url=new URL(fixturePage.url());evidence.page={origin:url.origin,path:url.pathname};evidence.hookState=await fixturePage.locator('[data-tailcat="status"]').textContent().catch(()=>null);}}
finally {
 await browser?.close();
 if(depotProcess?.pid){try{depotProcess.kill('SIGTERM');}catch{}await new Promise(r=>setTimeout(r,500));try{depotProcess.kill('SIGKILL');}catch{}}
 if(delivery){try{await control('/stop',{id:delivery.id});}catch{evidence.ok=false;evidence.nativeCleanup='unconfirmed';}}
 web.closeAllConnections();await new Promise(resolve=>web.close(resolve));
}
process.stdout.write(JSON.stringify(evidence)+'\n');if(!evidence.ok)process.exitCode=1;
