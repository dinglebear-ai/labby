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
