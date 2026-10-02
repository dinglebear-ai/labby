import {HttpDecoder} from './http.mjs';
const encoder=new TextEncoder();
const safeHeader=value=>typeof value==='string'&&/^[\x21-\x7e]+$/.test(value);

/** One browser's transient MCP session. Does not grant native authority. */
export class TailcatClient {
 static async connect(capability,{dial=globalThis.tailcatDial,signal}={}){
  if(signal?.aborted||!capability||!safeHeader(capability.grant)||!safeHeader(capability.generation)
   ||!capability.peer?.privateKey||!Number.isFinite(capability.expiresAt)||capability.expiresAt<=Date.now()
   ||capability.expiresAt>Date.now()+15*60000||capability.port!==1||!/^tcp[A-Za-z0-9_-]+$/.test(capability.address))throw Error('Invalid or expired pairing');
  const url=new URL(capability.derpMapURL);if(url.protocol!=='https:'||url.username||url.password||url.hash)throw Error('Invalid relay map');
  if(typeof dial!=='function')throw Error('Tailcat WASM is unavailable');
  return new TailcatClient(capability,dial);
 }
 constructor(capability,dial){this.capability=capability;this.dial=dial;this.session=null;this.nextId=1;this.active=new Set();this.closed=false;this.pending=0;this.lifetime=new AbortController();}
 async request(method,params={}, {signal}={}){
  if(this.closed||this.capability.expiresAt<=Date.now()||signal?.aborted)throw Error('Connection closed or expired');
  if(this.pending>=8)throw Error('Connection capacity exceeded');
  if(!['initialize','notifications/initialized','tools/list','tools/call','ping'].includes(method))throw Error('Unsupported MCP method');
  const id=method.startsWith('notifications/')?undefined:this.nextId++;
  const body=JSON.stringify({jsonrpc:'2.0',...(id===undefined?{}:{id}),method,params});
  if(encoder.encode(body).length>1024*1024)throw Error('Request budget exceeded');
  this.pending++;let conn,timer,abortListener,lifetimeListener,rejectAbort;
  const aborted=new Promise((_,reject)=>{rejectAbort=()=>reject(Error('Operation cancelled'));abortListener=rejectAbort;lifetimeListener=rejectAbort;signal?.addEventListener('abort',abortListener,{once:true});this.lifetime.signal.addEventListener('abort',lifetimeListener,{once:true});});
  const deadline=()=>new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('Connection deadline exceeded')),Math.min(30000,Math.max(1,this.capability.expiresAt-Date.now())))});
  const wait=async promise=>{try{return await Promise.race([promise,aborted,deadline()])}finally{clearTimeout(timer)}};
  try{
   const dialing=this.dial({addr:this.capability.address,port:1,derpMapURL:this.capability.derpMapURL,privateKey:this.capability.peer.privateKey});
   let abandoned=false;dialing.then(c=>{if(abandoned)c.close()},()=>{});
   try{conn=await wait(dialing)}catch(e){abandoned=true;throw e}
   this.active.add(conn);
   const session=this.session?`Mcp-Session-Id: ${this.session}\r\n`:'';
   await wait(conn.write(encoder.encode(`POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer ${this.capability.grant}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nAccept-Encoding: identity\r\nMCP-Protocol-Version: 2025-03-26\r\n${session}Content-Length: ${encoder.encode(body).length}\r\nConnection: close\r\n\r\n${body}`)));
   const decoder=new HttpDecoder();
   for(;;){
    const chunk=await wait(conn.read());if(chunk===null)decoder.finish();else decoder.push(chunk);
    if(decoder.status&&(decoder.status<200||decoder.status>=300))throw Error(`MCP HTTP ${decoder.status}`);
    if(decoder.headers?.['mcp-session-id']){const s=decoder.headers['mcp-session-id'];if(!safeHeader(s))throw Error('Invalid MCP session');if(this.session&&s!==this.session)throw Error('MCP session changed');this.session=s;}
    for(const message of decoder.takeMessages()){
     if(message.jsonrpc!=='2.0')throw Error('Invalid MCP response');
     if(message.id!==undefined){if(message.id!==id)throw Error('Unexpected MCP response ID');return message;}
    }
    if(decoder.done){if(id===undefined)return null;throw Error('Missing MCP response')}
   }
  }finally{
   clearTimeout(timer);signal?.removeEventListener('abort',abortListener);this.lifetime.signal.removeEventListener('abort',lifetimeListener);
   if(conn){this.active.delete(conn);conn.close()}this.pending--;
  }
 }
 close(){if(this.closed)return;this.closed=true;this.lifetime.abort();for(const c of this.active)c.close();this.active.clear();this.session=null;this.capability=null;}
}
