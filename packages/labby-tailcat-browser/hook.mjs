import {loadTailcat} from './wasm.mjs';
import {TailcatClient} from './transport.mjs';

export async function readJsonResponse(response, maximum=4096) {
 if(!response.body)throw Error('Missing control response');
 const reader=response.body.getReader();const chunks=[];let length=0;
 try {
  for(;;){const {value,done}=await reader.read();if(done)break;length+=value.byteLength;
   if(length>maximum)throw Error('Control response exceeds budget');chunks.push(value);}
  const bytes=new Uint8Array(length);let offset=0;
  for(const chunk of chunks){bytes.set(chunk,offset);offset+=chunk.byteLength;}
  return JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes));
 } catch(error){await reader.cancel();throw error;} finally {reader.releaseLock();}
}

export function validateDelivery(packet, request, relay) {
 const keys=['version','id','address','port','grant','origin','peer','upstream','generation','expiresAt','derpMapURL'];
 if(!packet||Object.keys(packet).sort().join()!==keys.sort().join()
  ||packet.version!==1||packet.origin!==request.origin||packet.peer!==request.peer
  ||packet.upstream!==request.upstream||packet.derpMapURL!==relay
  ||typeof packet.id!=='string'||!packet.id||typeof packet.generation!=='string'||!packet.generation
  ||!Number.isFinite(packet.expiresAt)||packet.expiresAt<=Date.now()||packet.expiresAt>Date.now()+900000)
  throw Error('Delivery does not match this browser pairing');
}

const object=value=>value!==null&&typeof value==='object'&&!Array.isArray(value);
export function rpcResult(message) {
 if(!object(message)||message.jsonrpc!=='2.0'||'error' in message||!object(message.result))throw Error('Invalid MCP result');
 return message.result;
}
export async function discoverTools(client,{signal}={}) {
 const tools=[];const cursors=new Set();let cursor,bytes=0;
 const deadline=AbortSignal.timeout(30000);
 const combined=signal?AbortSignal.any([signal,deadline]):deadline;
 for(let page=0;page<8;page++) {
  const result=rpcResult(await client.request('tools/list',cursor===undefined?{}:{cursor},{signal:combined}));
  if(combined.aborted)throw Error('Discovery deadline exceeded');
  bytes+=new TextEncoder().encode(JSON.stringify(result)).byteLength;
  if(bytes>1024*1024||!Array.isArray(result.tools)||tools.length+result.tools.length>512)throw Error('Discovery budget exceeded');
  for(const tool of result.tools){if(!object(tool)||typeof tool.name!=='string'||!tool.name||!object(tool.inputSchema)||(tool.description!==undefined&&typeof tool.description!=='string'))throw Error('Invalid tool descriptor');tools.push({name:tool.name,description:tool.description});}
  cursor=result.nextCursor;
  if(cursor===undefined)return tools;
  if(typeof cursor!=='string'||!cursor||cursor.length>4096||cursors.has(cursor))throw Error('Invalid discovery cursor');
  cursors.add(cursor);
 }
 throw Error('Discovery page budget exceeded');
}

/** Portable LiveView DOM island. The private key and sealed bearer stay in memory. */
export function tailcatHook({csrfToken, assetRoot='/assets/tailcat/',fetcher=fetch,
 load=loadTailcat, connect=TailcatClient.connect.bind(TailcatClient)}={}) {
 return {
  mounted() {
   this.epoch=0;this.listeners=[];this.pending=null;this.client=null;this.pairId=null;this.importing=false;
   this.listen=(element,event,callback)=>{element.addEventListener(event,callback);this.listeners.push(()=>element.removeEventListener(event,callback));};
   this.state=state=>{this.el.querySelector('[data-tailcat="status"]').textContent=state;this.pushEvent('connection_state',{state});};
   this.discard=id=>{if(id)void this.post(`/ui/sandboxes/pair/${encodeURIComponent(id)}/discard`,{}).catch(()=>{});};
   this.close=()=>{this.epoch++;clearTimeout(this.expiryTimer);this.operation?.abort();this.unsubscribe?.();this.unsubscribe=null;this.client?.close();this.client=null;this.pending=null;this.importing=false;this.discard(this.pairId);this.pairId=null;this.el.querySelector('[data-tailcat="tools"]').textContent='';};
   this.post=async(path,body,signal)=>{
    const response=await fetcher(path,{method:'POST',credentials:'same-origin',redirect:'error',
     headers:{'Content-Type':'application/json','X-CSRF-Token':csrfToken},body:JSON.stringify(body),signal:signal?AbortSignal.any([signal,AbortSignal.timeout(5000)]):AbortSignal.timeout(5000)});
    if(!response.ok)throw Error('Pairing denied');
    if(response.status===204)return;
    return readJsonResponse(response);
   };
   this.listen(this.el.querySelector('[data-tailcat="prepare"]'),'click',async()=>{
    this.close();const epoch=this.epoch;this.operation=new AbortController();this.state('connecting');
    try {
     if(location.protocol!=='https:')throw Error('HTTPS is required');
     const upstream=this.el.querySelector('[name="upstream"]').value;
     const response=await fetcher(assetRoot+'manifest.json',{credentials:'same-origin',redirect:'error',signal:AbortSignal.timeout(5000)});
     if(!response.ok)throw Error('Pinned assets unavailable');
     const manifest=await readJsonResponse(response);const tc=await load({baseURL:assetRoot,wasmSha256:manifest.wasmSha256,runtimeSha256:manifest.runtimeSha256});
     if(epoch!==this.epoch)return;
     const peer=tc.identity();const request={version:1,origin:location.origin,peer:peer.publicKey,upstream};
     const {id}=await this.post('/ui/sandboxes/pair',request);
     if(epoch!==this.epoch){this.discard(id);return;}
     if(typeof id!=='string'||!id)throw Error('Invalid pairing ID');this.pairId=id;
     this.pending={id,peer,request,tc,relay:manifest.derpMapURL};
     this.expiryTimer=setTimeout(()=>{this.close();this.state('expired');},300000);
     const url=URL.createObjectURL(new Blob([JSON.stringify(request)],{type:'application/json'}));
     const link=document.createElement('a');link.href=url;link.download='labby-pairing.json';link.click();URL.revokeObjectURL(url);
     this.state('offline');
     this.el.querySelector('[data-tailcat="status"]').textContent='Approve the downloaded request with labby tailcat pair, then import its delivery file.';
    } catch {if(epoch===this.epoch){this.close();this.state('failed');}}
   });
   this.listen(this.el.querySelector('input[type="file"]'),'change',async event=>{
    const file=event.target.files?.[0];event.target.value='';const pending=this.pending;const epoch=this.epoch;
    if(this.importing||!pending)return;
    if(!file||file.size>16384){this.state('failed');return;}
    this.importing=true;const signal=this.operation.signal;
    try {
     const text=await file.text();if(epoch!==this.epoch)return;
     const packet=JSON.parse(text);validateDelivery(packet,pending.request,pending.relay);
     const descriptor=Object.fromEntries(['version','origin','peer','upstream','generation','expiresAt'].map(k=>[k,packet[k]]));
     await this.post(`/ui/sandboxes/pair/${encodeURIComponent(pending.id)}/confirm`,descriptor,signal);
     if(epoch!==this.epoch)return;
     // Consumption is one-use; failures require a fresh browser key and native approval.
     clearTimeout(this.expiryTimer);this.pending=null;this.state('connecting');
     const client=await connect({...packet,peer:pending.peer},{createSession:pending.tc.createSession,signal});
     if(epoch!==this.epoch){client.close();return;}this.client=client;
     this.unsubscribe=client.onClose(()=>{if(epoch===this.epoch&&this.client===client){this.close();this.state('failed');}});
     const initialized=rpcResult(await client.request('initialize',{protocolVersion:'2025-03-26',capabilities:{},clientInfo:{name:'depot',version:'1'}},{signal}));
     if(typeof initialized.protocolVersion!=='string'||!object(initialized.capabilities)||!object(initialized.serverInfo)||typeof initialized.serverInfo.name!=='string'||typeof initialized.serverInfo.version!=='string')throw Error('Invalid initialization');
     await client.request('notifications/initialized',{}, {signal});const tools=await discoverTools(client,{signal});
     if(epoch!==this.epoch)return;
     this.el.querySelector('[data-tailcat="tools"]').textContent=JSON.stringify(tools,null,2);
     this.state('ready');this.expiryTimer=setTimeout(()=>{this.close();this.state('expired');},Math.max(0,packet.expiresAt-Date.now()));
    } catch {if(epoch===this.epoch){this.close();this.state('failed');}}
   });
   this.listen(this.el.querySelector('[data-tailcat="disconnect"]'),'click',()=>{this.close();this.state('offline');});
   this.listen(window,'pagehide',()=>this.close());
  },
  updated() {},
  disconnected() {this.close();},
  reconnected() {this.state('offline');},
  destroyed() {this.close();this.listeners.forEach(remove=>remove());this.listeners=[];},
 };
}
