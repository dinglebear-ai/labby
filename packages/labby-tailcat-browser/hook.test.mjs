import test from 'node:test';
import assert from 'node:assert/strict';
import {validateDelivery} from './hook.mjs';
const peer='nodekey:'+'a'.repeat(64);
const request={version:1,origin:'https://depot.example',peer,upstream:'microsandbox'};
const packet=()=>({...request,id:'native-session',address:'tcpAddress',port:1,grant:'sealed',generation:'native-generation',expiresAt:Date.now()+60000,derpMapURL:'https://relay.example/map'});
test('delivery is bound to current origin, peer, upstream and approved relay',()=>{
 assert.doesNotThrow(()=>validateDelivery(packet(),request,'https://relay.example/map'));
 for(const change of [{peer:'nodekey:'+'b'.repeat(64)},{origin:'https://other.example'},{upstream:'host'},{derpMapURL:'https://other.example/map'},{expiresAt:Date.now()-1},{privateKey:'secret'}]){
  assert.throws(()=>validateDelivery({...packet(),...change},request,'https://relay.example/map'));
 }
});

test('hook keeps its DOM island through updates and retires key custody on lifecycle exits', async()=>{
 const {tailcatHook}=await import('./hook.mjs');
 const previousWindow=globalThis.window;
 let closed=0,added=0,removed=0;
 const element={addEventListener(){added++;},removeEventListener(){removed++;},textContent:''};
 globalThis.window=element;
 try {
  const hook=tailcatHook();hook.el={querySelector(){return element;}};hook.pushEvent=()=>{};
  hook.mounted();assert.equal(added,4);
  hook.client={close(){closed++;}};hook.pending={peer:{privateKey:'memory-only'}};
  hook.updated();assert.equal(closed,0);assert.ok(hook.pending);
  hook.disconnected();assert.equal(closed,1);assert.equal(hook.pending,null);
  hook.destroyed();assert.equal(closed,1);assert.equal(removed,4);assert.equal(hook.listeners.length,0);
 } finally {globalThis.window=previousWindow;}
});

test('JSON control responses stop reading at their byte budget', async()=>{
 const {readJsonResponse}=await import('./hook.mjs');
 let cancelled=false;
 const response=new Response(new ReadableStream({start(controller){controller.enqueue(new Uint8Array(4097));},cancel(){cancelled=true;}}));
 await assert.rejects(readJsonResponse(response));assert.equal(cancelled,true);
 assert.deepEqual(await readJsonResponse(new Response('{"id":"public"}')),{id:'public'});
});

import {tailcatHook,rpcResult,discoverTools} from './hook.mjs';
const rpc=result=>({jsonrpc:'2.0',id:1,result});
const tool={name:'sandbox_list',inputSchema:{type:'object'}};
test('discovery rejects RPC errors, malformed results and repeated cursors',async()=>{
 for(const result of [{jsonrpc:'2.0',error:{code:-32603}},rpc({}),rpc({tools:[{}]})])
  await assert.rejects(discoverTools({request:async()=>result}));
 let calls=0;
 await assert.rejects(discoverTools({request:async()=>{calls++;return rpc({tools:[tool],nextCursor:'repeat'});}}));
 assert.equal(calls,2);
 assert.throws(()=>rpcResult({jsonrpc:'2.0',result:{},error:{code:1}}));
});
test('discovery follows pages and enforces item, byte and page limits',async()=>{
 const cursors=[];
 assert.deepEqual(await discoverTools({request:async(method,params)=>{cursors.push(params);return rpc(cursors.length===1?{tools:[tool],nextCursor:'second'}:{tools:[{...tool,name:'sandbox_exec'}]});}}),[{name:'sandbox_list',description:undefined},{name:'sandbox_exec',description:undefined}]);
 assert.deepEqual(cursors,[{},{cursor:'second'}]);
 for(const result of [{tools:Array(513).fill(tool)},{tools:[{...tool,description:'x'.repeat(1024*1024)}]}])
  await assert.rejects(discoverTools({request:async()=>rpc(result)}));
 let calls=0;await assert.rejects(discoverTools({request:async()=>rpc({tools:[],nextCursor:String(++calls)})}));assert.equal(calls,8);
});

function deferred(){let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};}
function fixture(t,{prepare,confirm,connect,encrypted,open}={}) {
 const elements=new Map();
 const el=selector=>{if(!elements.has(selector))elements.set(selector,{textContent:'',value:'microsandbox',addEventListener(event,fn){this[event]=fn;},removeEventListener(){}});return elements.get(selector);};
 t.mock.method(globalThis.URL,'createObjectURL',()=> 'blob:test');t.mock.method(globalThis.URL,'revokeObjectURL',()=>{});
 const previous={window:globalThis.window,location:globalThis.location,document:globalThis.document};
 globalThis.window=el('window');globalThis.location={protocol:'https:',origin:request.origin};globalThis.document={createElement:()=>({click(){}})};
 const posts=[];let closeCallback;const client={request:async method=>method==='initialize'?rpc({protocolVersion:'2025-03-26',capabilities:{},serverInfo:{name:'labby',version:'1'}}):method==='tools/list'?rpc({tools:[tool]}):null,onClose(fn){closeCallback=fn;return ()=>{closeCallback=null;};},close(){this.closed=true;}};
 const hook=tailcatHook({fetcher:async(path,options)=>{
  if(path.endsWith('/delivery'))return new Response(JSON.stringify(encrypted??{version:1,sender:'nodekey:'+'b'.repeat(64),ciphertext:'sealed'}));
  if(!options?.method)return new Response(JSON.stringify({derpMapURL:'https://relay.example/map'}));
  posts.push(path);if(path==='/ui/sandboxes/pair')return new Response(JSON.stringify(await (prepare?.()??{id:'pair'})));
  if(path.endsWith('/confirm'))await confirm?.();return new Response(null,{status:204});
 },load:async()=>({identity:()=>({publicKey:peer,privateKey:'private',openDelivery:open}),createSession(){}}),connect:connect??(async()=>client)});
 hook.el={querySelector:el};hook.pushEvent=()=>{};hook.mounted();
 t.after(()=>{hook.destroyed();Object.assign(globalThis,previous);});
 return {hook,client,posts,el,prepare:()=>el('[data-tailcat="prepare"]').click(),import:file=>el('input[type="file"]').change({target:{files:[file],value:'delivery'}}),closed:()=>closeCallback?.()};
}
const delivery=()=>({size:1,text:async()=>JSON.stringify(packet())});
test('a second import cannot confirm twice or close the successful connection',async t=>{
 const waiting=deferred();const f=fixture(t,{confirm:()=>waiting.promise});await f.prepare();
 const first=f.import(delivery());await new Promise(setImmediate);await f.import(delivery());waiting.resolve();await first;
 assert.equal(f.posts.filter(p=>p.endsWith('/confirm')).length,1);assert.equal(f.el('[data-tailcat="status"]').textContent,'ready');assert.equal(f.client.closed,undefined);
 f.closed();assert.equal(f.el('[data-tailcat="status"]').textContent,'failed');assert.equal(f.el('[data-tailcat="tools"]').textContent,'');
});
test('disconnect during delivery reading cannot confirm stale approval',async t=>{
 const waiting=deferred();const f=fixture(t);await f.prepare();const importing=f.import({size:1,text:()=>waiting.promise});
 f.el('[data-tailcat="disconnect"]').click();waiting.resolve(JSON.stringify(packet()));await importing;
 assert.equal(f.posts.filter(p=>p.endsWith('/confirm')).length,0);assert.ok(f.posts.some(p=>p.endsWith('/discard')));
});
test('late prepare responses are discarded after disconnect',async t=>{
 const waiting=deferred();const f=fixture(t,{prepare:()=>waiting.promise});const preparing=f.prepare();await new Promise(setImmediate);
 f.el('[data-tailcat="disconnect"]').click();waiting.resolve({id:'late'});await preparing;assert.ok(f.posts.includes('/ui/sandboxes/pair/late/discard'));
});
test('initialization RPC errors cannot produce ready',async t=>{
 const f=fixture(t);await f.prepare();f.client.request=async()=>({jsonrpc:'2.0',error:{code:-32603}});await f.import(delivery());
 assert.equal(f.el('[data-tailcat="status"]').textContent,'failed');assert.equal(f.client.closed,true);
});
test('discovery propagates cancellation and rejects late results',async()=>{
 const controller=new AbortController();let received;
 await assert.rejects(discoverTools({request:async(_method,_params,{signal})=>{received=signal;controller.abort();return rpc({tools:[tool]});}},{signal:controller.signal}));
 assert.equal(received.aborted,true);
});

test('automatic rendezvous opens encrypted delivery and validates pairing binding',async t=>{
 const opened=[];
 const f=fixture(t,{prepare:()=>({id:'p'.repeat(43),code:'a'.repeat(43)}),open:async(sender,ciphertext)=>{opened.push([sender,ciphertext]);return {...packet(),pairingId:'p'.repeat(43)};}});
 await f.prepare();await new Promise(setImmediate);await new Promise(setImmediate);
 assert.equal(opened.length,1);
 assert.equal(f.el('[data-tailcat="status"]').textContent,'ready');
 assert.match(f.el('[data-tailcat="command"]').textContent,/--rendezvous https:\/\/depot.example --pairing-id p{43}/);
 assert.equal(f.posts.filter(p=>p.endsWith('/confirm')).length,1);
});
test('encrypted receipt with wrong pairing cannot confirm',async t=>{
 const f=fixture(t,{prepare:()=>({id:'p'.repeat(43),code:'a'.repeat(43)}),open:async()=>({...packet(),pairingId:'other'})});
 await f.prepare();await new Promise(setImmediate);await new Promise(setImmediate);
 assert.equal(f.posts.filter(p=>p.endsWith('/confirm')).length,0);
 assert.equal(f.el('[data-tailcat="status"]').textContent,'failed');
});
test('disconnect retires an in-flight encrypted opening',async t=>{
 const waiting=deferred();
 const f=fixture(t,{prepare:()=>({id:'p'.repeat(43),code:'a'.repeat(43)}),open:()=>waiting.promise});
 await f.prepare();await new Promise(setImmediate);
 f.el('[data-tailcat="disconnect"]').click();waiting.resolve({...packet(),pairingId:'p'.repeat(43)});await new Promise(setImmediate);
 assert.equal(f.posts.filter(p=>p.endsWith('/confirm')).length,0);
 assert.equal(f.el('[data-tailcat="code"]').textContent,'');
});
test('rendezvous polling bounds requests and promptly observes cancellation',async()=>{
 const {pollDelivery,publicRequestFingerprint}=await import('./hook.mjs');
 const controller=new AbortController();let calls=0;
 await assert.rejects(pollDelivery({id:'pair',signal:controller.signal,fetcher:async()=>{calls++;return new Response(null,{status:202});},wait:async()=>{}}),/expired/);
 assert.equal(calls,150);
 const pending=pollDelivery({id:'pair',signal:controller.signal,fetcher:async()=>new Response(null,{status:202})});
 await new Promise(setImmediate);controller.abort();await assert.rejects(pending,/canceled/);
 const first=await publicRequestFingerprint('pair',request);
 assert.equal(first.length,64);assert.notEqual(first,await publicRequestFingerprint('other',request));
});
